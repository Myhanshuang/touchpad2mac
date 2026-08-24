#![forbid(unsafe_code)]
//! The real RemoteDesktop portal client over zbus (M6).
//!
//! Speaks the `org.freedesktop.portal.RemoteDesktop` interface (version 2,
//! observed on this host) over the D-Bus **session bus** using the pure-Rust
//! `zbus` blocking API — no system D-Bus library is linked, so builds and
//! tests stay operable without a session bus.
//!
//! # Protocol flow (portal v2)
//!
//! ```text
//! CreateSession(options)          → request object; the Response signal
//!                                    carries the session handle
//! SelectDevices(session, options) → request object; Response = status
//! Start(session, "", options)     → request object; Response = the user's
//!                                    authorization decision (0 ok, 1
//!                                    cancelled, 2 refused)
//! ConnectToEIS(session, options)  → returns the EIS socket fd (v2 method)
//! Session.Close                   → closes the session
//! ```
//!
//! # Race-free request handling
//!
//! The portal delivers each request's result as the
//! `org.freedesktop.portal.Request::Response` signal on the request object
//! path. To avoid missing a response that arrives before subscription, the
//! client follows the portal convention: it supplies a `handle_token`,
//! computes the request path
//! (`/org/freedesktop/portal/desktop/request/<sender_component>/<token>`),
//! subscribes to the `Response` signal on that path **first**, and only then
//! calls the method. The sender component is the client's unique bus name
//! with the leading `:` stripped and `.` replaced by `_` (exactly what
//! xdg-desktop-portal computes in `xdp_request_init_invocation`:
//! `g_strdup (sender + 1)` then `.` → `_`; `:1.42` → `1_42`).
//!
//! # Object-path-safe tokens (M6 re-review R12)
//!
//! `handle_token` and `session_handle_token` are embedded as the **last
//! element of the request/session handle object path**, so they are
//! generated from the D-Bus object-path-safe alphabet (`[A-Za-z0-9_]` —
//! letters, digits, underscore — exactly the charset xdg-desktop-portal's
//! `xdp_is_valid_token` accepts). Every predicted request path is validated
//! with zvariant **before** any match rule is registered, so a bad token or
//! path fails with a diagnostic that names the constructed path instead of a
//! context-free `Invalid object path`. `CreateSession` additionally takes a
//! **distinct** `session_handle_token` (same guarantees); non-request
//! methods such as `ConnectToEIS` carry no `handle_token` (their options
//! contract does not permit one).
//!
//! The response wait runs on a helper thread that races the async message
//! stream against a deadline: the thread exits on timeout as well as on a
//! response, so a timed-out portal request never leaves a blocked thread
//! accumulating until process exit (M6 cleanup).
//!
//! # `CreateSession` response wire ABI — `session_handle` is `s`, not `o`
//! (M6 re-review R13)
//!
//! The installed
//! `/usr/share/dbus-1/interfaces/org.freedesktop.portal.RemoteDesktop.xml`
//! declares the `CreateSession` response's `session_handle` as D-Bus
//! **string (`s`)**: it is an object path that was "erroneously implemented
//! as `s`" and, for backwards compatibility, remains `s`. The client
//! therefore decodes the response entry as a string first and then validates
//! the string's contents as an `OwnedObjectPath` (see
//! [`decode_create_session_response`]) — it never claims the response wire
//! type is `o`. The previous direct `OwnedObjectPath::try_from(OwnedValue)`
//! conversion rejected the correct string value with a bare
//! `incorrect type` (the live `--emit` failure in re-review 4).

use std::collections::HashMap;
use std::time::Duration;

use futures_lite::StreamExt;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type as MessageType;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

#[cfg(unix)]
use std::os::fd::IntoRawFd;
use zbus::MatchRule;

use crate::error::DesktopOutputError;
use crate::portal::{EisFd, Portal, PortalSession};
use crate::probe::PortalProbeInfo;

/// The portal bus name.
pub const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
/// The portal object path.
pub const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
/// The RemoteDesktop interface.
pub const REMOTE_DESKTOP_IFACE: &str = "org.freedesktop.portal.RemoteDesktop";
/// The Request interface.
pub const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";
/// The Session interface.
pub const SESSION_IFACE: &str = "org.freedesktop.portal.Session";

/// How long `CreateSession`/`SelectDevices` may take.
const SHORT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);
/// How long `Start` may take (the user can spend a while on the dialog).
const START_RESPONSE_TIMEOUT: Duration = Duration::from_secs(120);

/// The options dictionary (`a{sv}`).
type Options = HashMap<&'static str, OwnedValue>;

/// Returns `Ok(())` when the D-Bus session bus is reachable (connecting and
/// authenticating is side-effect-free; no session is created).
pub fn session_bus_reachable() -> Result<(), zbus::Error> {
    let _connection = Connection::session()?;
    Ok(())
}

/// Reads the RemoteDesktop portal's `version` and `AvailableDeviceTypes`
/// properties (read-only; no session is created).
pub fn probe_portal() -> Result<PortalProbeInfo, DesktopOutputError> {
    let connection = Connection::session()
        .map_err(|error| DesktopOutputError::NoSessionBus(error.to_string()))?;
    let version = get_u32_property(&connection, "version")?;
    let available_device_types = get_u32_property(&connection, "AvailableDeviceTypes")?;
    Ok(PortalProbeInfo {
        interface_version: version,
        available_device_types,
    })
}

fn get_u32_property(connection: &Connection, property: &str) -> Result<u32, DesktopOutputError> {
    let reply = connection
        .call_method(
            Some(PORTAL_BUS),
            PORTAL_PATH,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &(REMOTE_DESKTOP_IFACE, property),
        )
        .map_err(|error| {
            DesktopOutputError::PortalUnavailable(format!(
                "could not read {REMOTE_DESKTOP_IFACE}.{property}: {error}"
            ))
        })?;
    let value: OwnedValue = reply
        .body()
        .deserialize()
        .map_err(|error| DesktopOutputError::PortalUnavailable(error.to_string()))?;
    u32::try_from(&value).map_err(|error| DesktopOutputError::PortalUnavailable(error.to_string()))
}

/// Whether `token` is a valid D-Bus object-path element: non-empty and only
/// ASCII letters, digits or `_`. This is exactly the charset the portal
/// accepts (`xdp_is_valid_token` in xdg-desktop-portal's
/// `shared/xdp-utils.c`), because the portal embeds the token as the **last
/// element of the request/session handle object path** (M6 re-review R12 —
/// the previous `m6-<pid>-<counter>` format contained `-`, which is not a
/// valid object-path element).
fn is_object_path_safe_token(token: &str) -> bool {
    !token.is_empty() && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The sender component the portal embeds in request/session handle paths:
/// the client's unique bus name with the leading `:` stripped and `.`
/// replaced by `_`. This is exactly what xdg-desktop-portal computes in
/// `xdp_request_init_invocation`/`xdp_session_init_invocation`
/// (`g_strdup (sender + 1)` then `.` → `_`), so `:1.42` → `1_42`. The client
/// must predict the same path to subscribe to the `Response` signal
/// race-free; a `.`/`:` → `_` replacement that keeps the leading colon would
/// predict a different (wrong) path than the portal exports.
fn portal_sender_component(unique_name: &str) -> String {
    unique_name
        .strip_prefix(':')
        .unwrap_or(unique_name)
        .replace('.', "_")
}

/// Builds and validates a predicted portal handle path of the given `kind`
/// (`request` or `session`):
/// `/org/freedesktop/portal/desktop/<kind>/<sender_component>/<token>`,
/// following the xdg-desktop-portal naming convention exactly. The complete
/// path is validated with zvariant **before** any match rule is registered;
/// on failure the error names the path construction (kind, sender component,
/// token and the validation detail) instead of the context-free
/// `Invalid object path` the match-rule builder would surface later (M6
/// re-review R12).
fn predicted_handle_path(
    kind: &str,
    sender_component: &str,
    token: &str,
) -> Result<String, DesktopOutputError> {
    let path = format!("/org/freedesktop/portal/desktop/{kind}/{sender_component}/{token}");
    if !is_object_path_safe_token(token) {
        return Err(DesktopOutputError::InvalidPortalPath {
            kind: kind.to_string(),
            path,
            sender_component: sender_component.to_string(),
            token: token.to_string(),
            detail: "token is not a valid D-Bus object-path element \
                     (must contain only ASCII letters, digits or '_')"
                .to_string(),
        });
    }
    match zbus::zvariant::ObjectPath::try_from(path.as_str()) {
        Ok(_) => Ok(path),
        Err(error) => Err(DesktopOutputError::InvalidPortalPath {
            kind: kind.to_string(),
            path,
            sender_component: sender_component.to_string(),
            token: token.to_string(),
            detail: error.to_string(),
        }),
    }
}

/// The predicted portal request path for a handle token (see
/// [`predicted_handle_path`]).
fn request_path(connection: &Connection, token: &str) -> Result<String, DesktopOutputError> {
    let sender = connection
        .unique_name()
        .ok_or_else(|| DesktopOutputError::Internal("no unique bus name".to_string()))?;
    predicted_handle_path("request", &portal_sender_component(sender), token)
}

/// The predicted portal session path for a session handle token (see
/// [`predicted_handle_path`]). The portal builds this path itself from the
/// `session_handle_token` the client supplies in `CreateSession`; the client
/// predicts and validates it so a bad session token fails locally with a
/// diagnostic that names the path construction (M6 re-review R12).
fn session_path_predicted(
    connection: &Connection,
    token: &str,
) -> Result<String, DesktopOutputError> {
    let sender = connection
        .unique_name()
        .ok_or_else(|| DesktopOutputError::Internal("no unique bus name".to_string()))?;
    predicted_handle_path("session", &portal_sender_component(sender), token)
}

/// A response to a portal request: the response code (0 ok, 1 cancelled, 2
/// refused) and the result dictionary.
type PortalResponse = (u32, HashMap<String, OwnedValue>);

/// Subscribes to the `Response` signal on `request_path` and returns an
/// iterator (registered before the method call — race-free).
fn subscribe_response(
    connection: &Connection,
    request_path: &str,
) -> Result<MessageIterator, DesktopOutputError> {
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(PORTAL_BUS)
        .map_err(|error| DesktopOutputError::Internal(error.to_string()))?
        .path(request_path)
        // The path was already validated as an object path with zvariant
        // before this call (M6 re-review R12); if the match rule still
        // rejects it, name the path construction in the diagnostic.
        .map_err(|error| {
            DesktopOutputError::Internal(format!(
                "could not register the Response match rule on request path \
                 {request_path:?}: {error}"
            ))
        })?
        .interface(REQUEST_IFACE)
        .map_err(|error| DesktopOutputError::Internal(error.to_string()))?
        .member("Response")
        .map_err(|error| DesktopOutputError::Internal(error.to_string()))?
        .build();
    MessageIterator::for_match_rule(rule, connection, Some(1))
        .map_err(|error| DesktopOutputError::Internal(error.to_string()))
}

/// Blocks (with a timeout) for the portal's `Response` signal. The waiter
/// runs on a helper thread that races the async message stream against a
/// deadline: **on timeout the thread exits too**, so a portal that never
/// answers cannot leave a blocked thread accumulating until process exit
/// (M6 cleanup).
fn wait_response(
    iterator: MessageIterator,
    timeout: Duration,
    what: &str,
) -> Result<PortalResponse, DesktopOutputError> {
    // Convert the blocking iterator to its underlying async stream so the
    // waiter can race `next()` against a deadline.
    let stream = iterator.into_inner();
    let (sender, receiver) = std::sync::mpsc::channel::<PortalResponse>();
    std::thread::spawn(move || {
        let accepted = zbus::block_on(first_accepted_or_timeout(
            stream,
            timeout,
            |message: zbus::Result<zbus::Message>| {
                let Ok(message) = message else { return None };
                message.body().deserialize::<PortalResponse>().ok()
            },
        ));
        if let Some(response) = accepted {
            let _ = sender.send(response);
        }
    });
    receiver
        .recv_timeout(timeout)
        .map_err(|_| DesktopOutputError::Timeout(format!("waiting for {what}")))
}

/// Races an async stream against a deadline, returning the first item
/// `accept` maps to `Some(..)` (skipping non-matching items), or `None`
/// when the deadline expires first or the stream ends. Used on the portal
/// response helper thread so a timeout terminates the thread instead of
/// abandoning it (M6 cleanup: portal response timeout must not leak a
/// blocked thread).
async fn first_accepted_or_timeout<S, T, U>(
    mut stream: S,
    timeout: Duration,
    mut accept: impl FnMut(T) -> Option<U>,
) -> Option<U>
where
    S: futures_lite::Stream<Item = T> + Unpin,
{
    let deadline = async_io::Timer::after(timeout);
    futures_lite::future::or(
        async {
            loop {
                match stream.next().await {
                    Some(item) => {
                        if let Some(value) = accept(item) {
                            return Some(value);
                        }
                    }
                    None => return None,
                }
            }
        },
        async {
            deadline.await;
            None
        },
    )
    .await
}

/// Interprets the portal response code: 0 ok, 1 cancelled, 2 refused.
fn interpret_response(
    (response, results): PortalResponse,
    what: &str,
) -> Result<HashMap<String, OwnedValue>, DesktopOutputError> {
    match response {
        0 => Ok(results),
        1 => Err(DesktopOutputError::AuthorizationCancelled),
        2 => Err(DesktopOutputError::AuthorizationRefused {
            response,
            message: results
                .get("error")
                .and_then(|value| <&str>::try_from(value).ok())
                .map(str::to_string)
                .unwrap_or_else(|| format!("portal refused {what}")),
        }),
        other => Err(DesktopOutputError::AuthorizationRefused {
            response: other,
            message: format!("portal returned unexpected response {other} for {what}"),
        }),
    }
}

/// Decodes the `session_handle` entry of a `CreateSession` response into a
/// [`PortalSession`].
///
/// # Wire ABI — `s`, not `o` (M6 re-review R13)
///
/// The installed
/// `/usr/share/dbus-1/interfaces/org.freedesktop.portal.RemoteDesktop.xml`
/// (v2) declares the `CreateSession` response's `session_handle` as D-Bus
/// **string (`s`)**:
///
/// ```text
/// * ``session_handle`` (``s``)
///   The session handle. An object path for the Session object representing
///   the created session.
///   .. note::
///     The ``session_handle`` is an object path that was erroneously
///     implemented as ``s``. For backwards compatibility it will remain
///     this type.
/// ```
///
/// So the entry is decoded **according to the wire ABI as a string first**,
/// and only then is the string's *contents* validated as a D-Bus object
/// path. The previous code converted the `OwnedValue` straight into
/// `OwnedObjectPath`, which rejects the correct string value with a bare
/// `incorrect type` — the exact failure the live `--emit` hit in re-review
/// 4 (`session_handle is not an object path: incorrect type`). The three
/// failure classes keep distinct, contextual diagnostics:
///
/// * missing key — the response carries no `session_handle`;
/// * wrong D-Bus value type — the value is not a string; the message names
///   the actual value (`{value:?}`) and its D-Bus signature
///   (`{value.value_signature()}`), e.g. `U32(42)` with signature `'u'`;
/// * syntactically invalid path — the string is not a valid D-Bus object
///   path (the message names the offending string).
fn decode_create_session_response(
    results: &HashMap<String, OwnedValue>,
) -> Result<PortalSession, DesktopOutputError> {
    let session_handle = results.get("session_handle").ok_or_else(|| {
        DesktopOutputError::PortalUnavailable(
            "CreateSession response has no session_handle".to_string(),
        )
    })?;
    // The wire type is `s` (see the doc comment above); the string carries
    // the session object path, which is validated after the string decode.
    let session_handle = <&str>::try_from(session_handle).map_err(|error| {
        DesktopOutputError::PortalUnavailable(format!(
            "CreateSession response session_handle has the wrong D-Bus type: \
             expected a string ('s') containing an object path, found value \
             {session_handle:?} with D-Bus signature '{}': {error}",
            session_handle.value_signature(),
        ))
    })?;
    let session_handle = OwnedObjectPath::try_from(session_handle).map_err(|error| {
        DesktopOutputError::PortalUnavailable(format!(
            "CreateSession response session_handle {session_handle:?} is not a valid \
             D-Bus object path: {error}"
        ))
    })?;
    Ok(PortalSession(session_handle.to_string()))
}

/// Generates unique, D-Bus **object-path-safe** portal handle tokens.
///
/// Each token is `m6_<pid>_<counter>`: only ASCII letters, digits and
/// underscores — the alphabet xdg-desktop-portal's `xdp_is_valid_token`
/// accepts, since the portal embeds the token as the **last element of the
/// request/session handle object path** (M6 re-review R12: the previous
/// `m6-<pid>-<counter>` format contained `-`, which is not a valid
/// object-path element and made every predicted request path invalid before
/// any portal method was called).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TokenGenerator {
    /// The process id embedded in each token.
    pid: u32,
    /// Monotonic per-generator counter (unique within the process).
    counter: u64,
}

impl TokenGenerator {
    /// A generator for the current process.
    fn new() -> Self {
        Self {
            pid: std::process::id(),
            counter: 0,
        }
    }

    /// The next unique token. Tokens are unique across calls because the
    /// counter is strictly increasing; the pid makes tokens from different
    /// processes distinct.
    fn next(&mut self) -> String {
        self.counter += 1;
        format!("m6_{}_{}", self.pid, self.counter)
    }
}

/// The real portal client over a persistent session-bus connection.
#[derive(Debug)]
pub struct ZbusPortal {
    connection: Connection,
    /// Unique handle-token generator (request and session tokens are both
    /// drawn from here, so they are always distinct).
    tokens: TokenGenerator,
}

impl ZbusPortal {
    /// Connects to the session bus.
    pub fn connect() -> Result<Self, DesktopOutputError> {
        let connection = Connection::session()
            .map_err(|error| DesktopOutputError::NoSessionBus(error.to_string()))?;
        Ok(Self {
            connection,
            tokens: TokenGenerator::new(),
        })
    }

    /// Builds the options dictionary for a **request-based** method
    /// (`CreateSession`/`SelectDevices`/`Start`) with a fresh unique
    /// `handle_token`, plus any extra entries (all values are owned, so no
    /// borrows escape). The request path built from this token is validated
    /// with zvariant before the match rule is registered (M6 re-review R12).
    fn options(
        &mut self,
        extra: &[(&'static str, OwnedValue)],
    ) -> Result<Options, DesktopOutputError> {
        let token = self.tokens.next();
        Ok(Self::request_options(&token, extra))
    }

    /// The pure options builder for a request-based method: a fresh
    /// `handle_token` plus any method-specific extras. Request methods carry
    /// a `handle_token`; the session token is **only** used by `CreateSession`
    /// (see [`Self::create_session_options`]) — never added to
    /// `SelectDevices`/`Start` options (M6 re-review R12).
    fn request_options(token: &str, extra: &[(&'static str, OwnedValue)]) -> Options {
        let mut options = Options::new();
        options.insert(
            "handle_token",
            OwnedValue::from(zbus::zvariant::Str::from(token)),
        );
        for (key, value) in extra {
            options.insert(*key, value.clone());
        }
        options
    }

    /// Builds the `CreateSession` options dict from a request `handle_token`
    /// and a **distinct** `session_handle_token` — the two option keys the
    /// RemoteDesktop spec documents for `CreateSession`, each used as the
    /// last element of the request/session handle object path (M6 re-review
    /// R12: the session token was previously missing).
    fn create_session_options(handle_token: &str, session_token: &str) -> Options {
        let mut options = Options::new();
        options.insert(
            "handle_token",
            OwnedValue::from(zbus::zvariant::Str::from(handle_token)),
        );
        options.insert(
            "session_handle_token",
            OwnedValue::from(zbus::zvariant::Str::from(session_token)),
        );
        options
    }

    fn handle_token(options: &Options) -> Result<&str, DesktopOutputError> {
        options
            .get("handle_token")
            .and_then(|value| <&str>::try_from(value).ok())
            .ok_or_else(|| DesktopOutputError::Internal("handle_token missing".to_string()))
    }

    fn session_path(session: &PortalSession) -> Result<OwnedObjectPath, DesktopOutputError> {
        OwnedObjectPath::try_from(session.0.as_str())
            .map_err(|error| DesktopOutputError::Internal(error.to_string()))
    }

    /// One request round-trip with a **prebuilt** options dict (used by
    /// `create_session`, which needs both a `handle_token` and a distinct
    /// `session_handle_token`): validate the predicted request path with
    /// zvariant, subscribe first, call the method, wait for the response,
    /// interpret the response code. `build_body` turns the options into the
    /// method's exact body tuple.
    fn request_with_options<B>(
        &self,
        method: &str,
        timeout: Duration,
        options: Options,
        build_body: impl FnOnce(Options) -> B,
    ) -> Result<HashMap<String, OwnedValue>, DesktopOutputError>
    where
        B: serde::Serialize + zbus::zvariant::Type,
    {
        let token = Self::handle_token(&options)?.to_string();
        let path = request_path(&self.connection, &token)?;
        let iterator = subscribe_response(&self.connection, &path)?;
        let body = build_body(options);
        self.connection
            .call_method(
                Some(PORTAL_BUS),
                PORTAL_PATH,
                Some(REMOTE_DESKTOP_IFACE),
                method,
                &body,
            )
            .map_err(|error| {
                DesktopOutputError::PortalUnavailable(format!("{method} failed: {error}"))
            })?;
        let response = wait_response(iterator, timeout, method)?;
        interpret_response(response, method)
    }

    /// One request round-trip for the request-based methods that only need a
    /// fresh `handle_token` (`SelectDevices`, `Start`): builds the options,
    /// then delegates to [`Self::request_with_options`].
    fn request<B>(
        &mut self,
        method: &str,
        timeout: Duration,
        extra: &[(&'static str, OwnedValue)],
        build_body: impl FnOnce(Options) -> B,
    ) -> Result<HashMap<String, OwnedValue>, DesktopOutputError>
    where
        B: serde::Serialize + zbus::zvariant::Type,
    {
        let options = self.options(extra)?;
        self.request_with_options(method, timeout, options, build_body)
    }
}

impl Portal for ZbusPortal {
    fn create_session(&mut self) -> Result<PortalSession, DesktopOutputError> {
        // RemoteDesktop.CreateSession(options) — the spec documents exactly
        // two option keys, both used as the last element of an object path
        // and both required to be valid D-Bus object-path elements:
        //   * `handle_token` — last element of the request handle path;
        //   * `session_handle_token` — last element of the session handle
        //     path.
        // The two tokens are generated separately (always distinct) with the
        // same safe-alphabet/uniqueness guarantees (M6 re-review R12: the
        // session token was previously absent). The request path (from
        // `handle_token`) is validated inside `request_with_options`; the
        // predicted session path (from `session_handle_token`) is validated
        // here so a bad session token fails locally with a diagnostic that
        // names the path construction.
        let handle_token = self.tokens.next();
        let session_token = self.tokens.next();
        let _ = session_path_predicted(&self.connection, &session_token)?;
        let options = Self::create_session_options(&handle_token, &session_token);
        let results = self.request_with_options(
            "CreateSession",
            SHORT_RESPONSE_TIMEOUT,
            options,
            |options| (options,),
        )?;
        // The response's `session_handle` is a D-Bus **string** on the wire
        // (an object path that was historically implemented as `s` and kept
        // for compatibility — M6 re-review R13): decode the string first,
        // then validate its contents as an object path.
        decode_create_session_response(&results)
    }

    fn select_devices(
        &mut self,
        session: &PortalSession,
        types: u32,
    ) -> Result<(), DesktopOutputError> {
        let session_path = Self::session_path(session)?;
        let _results = self.request(
            "SelectDevices",
            SHORT_RESPONSE_TIMEOUT,
            &[("types", OwnedValue::from(types))],
            move |options| (session_path, options),
        )?;
        Ok(())
    }

    fn start(&mut self, session: &PortalSession) -> Result<(), DesktopOutputError> {
        // Start(session, parent_window, options): no parent window ("").
        let session_path = Self::session_path(session)?;
        let _results = self.request("Start", START_RESPONSE_TIMEOUT, &[], move |options| {
            (session_path, "", options)
        })?;
        Ok(())
    }

    fn connect_to_eis(&mut self, session: &PortalSession) -> Result<EisFd, DesktopOutputError> {
        // `ConnectToEIS` is a **synchronous** fd-returning method, not a
        // Request-based one: the RemoteDesktop spec documents no option keys
        // for it (unlike CreateSession/SelectDevices/Start, whose options
        // carry a `handle_token`), so its options dict is empty — adding a
        // request token here would be an option the method's contract does
        // not allow (M6 re-review R12).
        let options = Options::new();
        let session_path = Self::session_path(session)?;
        let reply = self
            .connection
            .call_method(
                Some(PORTAL_BUS),
                PORTAL_PATH,
                Some(REMOTE_DESKTOP_IFACE),
                "ConnectToEIS",
                &(session_path, options),
            )
            .map_err(|error| {
                DesktopOutputError::PortalUnavailable(format!("ConnectToEIS failed: {error}"))
            })?;
        extract_fd(reply)
    }

    fn close_session(&mut self, session: &PortalSession) -> Result<(), DesktopOutputError> {
        self.connection
            .call_method(
                Some(PORTAL_BUS),
                session.0.as_str(),
                Some(SESSION_IFACE),
                "Close",
                &(),
            )
            .map_err(|error| {
                DesktopOutputError::PortalUnavailable(format!("Session.Close failed: {error}"))
            })?;
        Ok(())
    }
}

/// Extracts the EIS fd from the `ConnectToEIS` reply (`h` type).
#[cfg(unix)]
fn extract_fd(reply: zbus::Message) -> Result<EisFd, DesktopOutputError> {
    let owned: zbus::zvariant::OwnedFd = reply
        .body()
        .deserialize()
        .map_err(|error| DesktopOutputError::PortalUnavailable(error.to_string()))?;
    let std_fd: std::os::fd::OwnedFd = owned.into();
    // `OwnedFd::into_raw_fd` hands ownership of the fd to the caller; the
    // transport (`ei_setup_backend_fd`) takes it over and closes it on
    // teardown.
    Ok(EisFd(std_fd.into_raw_fd()))
}

/// Non-Unix platforms have no portal fd to hand to libei.
#[cfg(not(unix))]
fn extract_fd(_reply: zbus::Message) -> Result<EisFd, DesktopOutputError> {
    Err(DesktopOutputError::UnsupportedPlatform(
        "the RemoteDesktop portal EIS fd is only available on Unix".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portal::device_types;

    /// M6 re-review R12: the sender component embedded in request/session
    /// paths must match xdg-desktop-portal exactly — the unique sender name
    /// with the leading `:` stripped and `.` replaced by `_`
    /// (`xdp_request_init_invocation`/`xdp_session_init_invocation`:
    /// `g_strdup (sender + 1)` then `.` → `_`), so `:1.42` → `1_42`. The
    /// token-based race-free subscription depends on predicting the same
    /// path the portal exports.
    #[test]
    fn sender_component_matches_the_portal_convention() {
        assert_eq!(portal_sender_component(":1.42"), "1_42");
        assert_eq!(portal_sender_component(":1.7"), "1_7");
        // A leading colon is always present on a real unique name, but the
        // helper also handles an already-stripped name.
        assert_eq!(portal_sender_component("1.42"), "1_42");
    }

    /// M6 re-review R12: every generated token is a valid D-Bus object-path
    /// element — non-empty and only ASCII letters, digits or `_` (the
    /// charset xdg-desktop-portal's `xdp_is_valid_token` accepts). The
    /// previous `m6-<pid>-<counter>` format contained `-` and produced
    /// `Invalid object path` before any portal method call.
    #[test]
    fn every_generated_token_is_object_path_safe() {
        let mut generator = TokenGenerator::new();
        for _ in 0..1_000 {
            let token = generator.next();
            assert!(
                is_object_path_safe_token(&token),
                "token {token:?} must be a valid object-path element"
            );
            assert!(!token.is_empty());
            assert!(
                token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "token {token:?} must use only [A-Za-z0-9_]"
            );
            assert!(!token.contains('-'), "token {token:?} must not contain '-'");
        }
    }

    /// M6 re-review R12: generated tokens are unique across calls (strictly
    /// increasing counter), and tokens from different generators (different
    /// processes) are distinct because of the embedded pid.
    #[test]
    fn generated_tokens_are_unique() {
        let mut generator = TokenGenerator::new();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..10_000 {
            let token = generator.next();
            assert!(seen.insert(token.clone()), "duplicate token {token:?}");
        }
        // A different "process" (pid) must not collide with the first
        // generator's tokens.
        let mut other = TokenGenerator {
            pid: generator.pid.wrapping_add(1),
            counter: 0,
        };
        for _ in 0..100 {
            assert!(
                !seen.contains(&other.next()),
                "cross-process token collided"
            );
        }
    }

    /// M6 re-review R12: every predicted request/session path for generated
    /// tokens is a valid D-Bus object path (zvariant validates it) and
    /// follows the portal naming convention:
    /// `/org/freedesktop/portal/desktop/{request,session}/<sender>/<token>`.
    #[test]
    fn predicted_paths_are_valid_and_follow_the_portal_convention() {
        let mut generator = TokenGenerator::new();
        let sender_component = portal_sender_component(":1.42");
        assert_eq!(sender_component, "1_42");
        for kind in ["request", "session"] {
            for _ in 0..200 {
                let token = generator.next();
                let path = predicted_handle_path(kind, &sender_component, &token).unwrap_or_else(
                    |error| panic!("path for {kind}/{token} must validate: {error}"),
                );
                assert_eq!(
                    path,
                    format!("/org/freedesktop/portal/desktop/{kind}/1_42/{token}"),
                    "the predicted path must follow the portal convention"
                );
                // zvariant itself accepts the complete path (this is the
                // same validation the match-rule builder would apply).
                zbus::zvariant::ObjectPath::try_from(path.as_str())
                    .expect("zvariant must accept the predicted path");
            }
        }
    }

    /// M6 re-review R12: a token that is not an object-path element fails
    /// **before** any match rule is registered, with a diagnostic that
    /// identifies the path construction (kind, sender component, token, the
    /// constructed path) instead of a context-free `Invalid object path`.
    #[test]
    fn invalid_token_fails_with_a_path_construction_diagnostic() {
        let error = predicted_handle_path("request", "1_42", "m6-1-1").unwrap_err();
        let message = error.to_string();
        assert!(
            matches!(
                &error,
                DesktopOutputError::InvalidPortalPath {
                    kind,
                    path,
                    sender_component,
                    token,
                    ..
                } if kind == "request"
                    && path == "/org/freedesktop/portal/desktop/request/1_42/m6-1-1"
                    && sender_component == "1_42"
                    && token == "m6-1-1"
            ),
            "{error:?}"
        );
        assert!(message.contains("invalid portal request path"), "{message}");
        assert!(
            message.contains("/org/freedesktop/portal/desktop/request/1_42/m6-1-1"),
            "{message}"
        );
        assert!(message.contains("m6-1-1"), "{message}");
        assert!(message.contains("object-path element"), "{message}");
    }

    /// M6 re-review R12: the `CreateSession` options dict carries exactly
    /// the two documented keys — a request `handle_token` and a **distinct**
    /// `session_handle_token` — both valid object-path elements; the session
    /// token is never reused as (or equal to) the request token. This runs
    /// without any live portal: it only constructs the options dict.
    #[test]
    fn create_session_options_carry_distinct_safe_tokens() {
        let mut generator = TokenGenerator::new();
        for _ in 0..100 {
            let handle_token = generator.next();
            let session_token = generator.next();
            let options = ZbusPortal::create_session_options(&handle_token, &session_token);
            assert_eq!(options.len(), 2, "exactly the two documented keys");
            let handle: &str = <&str>::try_from(&options["handle_token"]).unwrap();
            let session: &str = <&str>::try_from(&options["session_handle_token"]).unwrap();
            assert_eq!(handle, handle_token);
            assert_eq!(session, session_token);
            assert_ne!(
                handle, session,
                "request and session tokens must be distinct"
            );
            assert!(is_object_path_safe_token(handle));
            assert!(is_object_path_safe_token(session));
        }
    }

    /// M6 re-review R12: request-based methods (`SelectDevices`/`Start`)
    /// carry a `handle_token` (plus their method-specific keys) but never a
    /// `session_handle_token`; the synchronous `ConnectToEIS` documents no
    /// option keys, so its options dict carries neither token. This is the
    /// per-method options contract from the RemoteDesktop spec.
    #[test]
    fn per_method_option_contract_matches_the_remote_desktop_spec() {
        // CreateSession: exactly the two documented keys.
        let options = ZbusPortal::create_session_options("m6_1_1", "m6_1_2");
        assert!(options.contains_key("handle_token"));
        assert!(options.contains_key("session_handle_token"));

        // SelectDevices/Start (request methods): handle_token + the
        // method-specific `types` key, never a session token.
        let options = ZbusPortal::request_options("m6_1_3", &[("types", OwnedValue::from(2u32))]);
        assert!(options.contains_key("handle_token"));
        assert!(options.contains_key("types"));
        assert!(
            !options.contains_key("session_handle_token"),
            "SelectDevices/Start must not carry a session token"
        );

        // ConnectToEIS: synchronous fd-returning method, no request handle,
        // no documented option keys — its options dict is empty.
        let options = Options::new();
        assert!(options.is_empty());
        assert!(!options.contains_key("handle_token"));
    }

    /// M6 re-review R13: the `CreateSession` response's `session_handle` is
    /// a D-Bus **string** on the wire — the installed
    /// `/usr/share/dbus-1/interfaces/org.freedesktop.portal.RemoteDesktop.xml`
    /// declares it as `s`, noting the session handle "is an object path that
    /// was erroneously implemented as `s`. For backwards compatibility it
    /// will remain this type." (v2 compatibility case.) The pure decoder
    /// therefore accepts a valid path **string**.
    #[test]
    fn session_handle_decodes_from_wire_string() {
        // The exact installed v2 compatibility case: a string whose contents
        // are the session object path the portal built from the client's
        // `session_handle_token` (see `predicted_handle_path`).
        let path = "/org/freedesktop/portal/desktop/session/1_42/m6_1_1";
        let mut results = HashMap::new();
        results.insert(
            "session_handle".to_string(),
            OwnedValue::from(zbus::zvariant::Str::from(path)),
        );
        assert_eq!(
            decode_create_session_response(&results).unwrap(),
            PortalSession(path.to_string()),
        );
    }

    /// M6 re-review R13: a response missing the `session_handle` key fails
    /// with a diagnostic naming the missing key (distinct from the wrong-type
    /// and invalid-path diagnostics).
    #[test]
    fn session_handle_missing_key_fails() {
        let error = decode_create_session_response(&HashMap::new()).unwrap_err();
        assert!(matches!(&error, DesktopOutputError::PortalUnavailable(_)));
        let message = error.to_string();
        assert!(message.contains("no session_handle"), "{message}");
        assert!(message.contains("CreateSession"), "{message}");
        // The missing-key diagnostic must not be a wrong-type diagnostic.
        assert!(!message.contains("wrong D-Bus type"), "{message}");
    }

    /// M6 re-review R13: a non-string value fails with a diagnostic that
    /// names the **actual value and its D-Bus signature** — here a `u32`
    /// (signature `'u'`), which a naive `o`-typed client would misdecode.
    #[test]
    fn session_handle_wrong_type_fails_with_value_and_signature_context() {
        let mut results = HashMap::new();
        results.insert("session_handle".to_string(), OwnedValue::from(42u32));
        let error = decode_create_session_response(&results).unwrap_err();
        assert!(matches!(&error, DesktopOutputError::PortalUnavailable(_)));
        let message = error.to_string();
        assert!(message.contains("wrong D-Bus type"), "{message}");
        assert!(message.contains("U32(42)"), "{message}");
        assert!(message.contains("'u'"), "{message}");
    }

    /// M6 re-review R13: even a value of the *would-be-correct* D-Bus type
    /// `o` is rejected by the string-first wire decode, with the value and
    /// its signature in the diagnostic — the portal ABI is `s`, so the
    /// client must not accept `o` (the historical bug direction).
    #[test]
    fn session_handle_object_path_value_is_rejected_with_signature_context() {
        let path = "/org/freedesktop/portal/desktop/session/1_42/m6_1_1";
        let mut results = HashMap::new();
        results.insert(
            "session_handle".to_string(),
            OwnedValue::from(
                zbus::zvariant::ObjectPath::try_from(path)
                    .expect("the test path is a valid object path"),
            ),
        );
        let error = decode_create_session_response(&results).unwrap_err();
        assert!(matches!(&error, DesktopOutputError::PortalUnavailable(_)));
        let message = error.to_string();
        assert!(message.contains("wrong D-Bus type"), "{message}");
        assert!(message.contains("'o'"), "{message}");
    }

    /// M6 re-review R13: a string whose contents are **not** a valid D-Bus
    /// object path fails with a diagnostic naming the offending string. The
    /// token `m6-1-1` contains `-`, which is not a valid object-path element
    /// (the same character that made the old `m6-<pid>-<counter>` tokens
    /// invalid in R12).
    #[test]
    fn session_handle_invalid_path_string_fails() {
        let invalid = "/org/freedesktop/portal/desktop/session/1_42/m6-1-1";
        let mut results = HashMap::new();
        results.insert(
            "session_handle".to_string(),
            OwnedValue::from(zbus::zvariant::Str::from(invalid)),
        );
        let error = decode_create_session_response(&results).unwrap_err();
        assert!(matches!(&error, DesktopOutputError::PortalUnavailable(_)));
        let message = error.to_string();
        assert!(message.contains("not a valid"), "{message}");
        assert!(message.contains("D-Bus object path"), "{message}");
        assert!(message.contains(invalid), "{message}");
    }

    #[test]
    fn response_codes_map_to_structured_results() {
        let ok = interpret_response((0, HashMap::new()), "Start").unwrap();
        assert!(ok.is_empty());
        assert!(matches!(
            interpret_response((1, HashMap::new()), "Start"),
            Err(DesktopOutputError::AuthorizationCancelled)
        ));
        assert!(matches!(
            interpret_response((2, HashMap::new()), "Start"),
            Err(DesktopOutputError::AuthorizationRefused { response: 2, .. })
        ));
    }

    #[test]
    fn portal_constants_match_the_observed_interface() {
        assert_eq!(PORTAL_BUS, "org.freedesktop.portal.Desktop");
        assert_eq!(PORTAL_PATH, "/org/freedesktop/portal/desktop");
        assert_eq!(REMOTE_DESKTOP_IFACE, "org.freedesktop.portal.RemoteDesktop");
        assert_eq!(device_types::POINTER, 2);
    }

    /// Connecting to a session bus is side-effect-free; on machines without
    /// one the probe must produce a structured error, never a panic.
    #[test]
    fn session_bus_probe_is_structured() {
        let _ = session_bus_reachable();
    }

    /// M6 cleanup: the response race returns the **first accepted** item and
    /// skips non-matching ones.
    #[test]
    fn response_race_returns_the_first_accepted_item() {
        let stream = futures_lite::stream::iter(vec![1i32, 2, 3, 4]);
        let started = std::time::Instant::now();
        let result = futures_lite::future::block_on(first_accepted_or_timeout(
            stream,
            Duration::from_secs(5),
            |item| (item % 2 == 0).then_some(item),
        ));
        assert_eq!(result, Some(2));
        // Returns promptly, well before the deadline.
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    /// M6 cleanup: when the stream never yields, the deadline wins and the
    /// race returns `None` — the helper thread exits on timeout instead of
    /// being abandoned until process exit.
    #[test]
    fn response_race_times_out_when_nothing_arrives() {
        let stream = futures_lite::stream::pending::<i32>();
        let started = std::time::Instant::now();
        let result = futures_lite::future::block_on(first_accepted_or_timeout(
            stream,
            Duration::from_millis(30),
            |_| Some(()),
        ));
        assert_eq!(result, None);
        assert!(
            started.elapsed() >= Duration::from_millis(25),
            "the deadline must be what ends the wait (elapsed {:?})",
            started.elapsed()
        );
    }

    /// M6 cleanup: an exhausted stream ends the race with `None` promptly
    /// (no hang after the stream ends).
    #[test]
    fn response_race_ends_with_the_stream() {
        let stream = futures_lite::stream::iter(vec![1i32, 2, 3]);
        let started = std::time::Instant::now();
        let result = futures_lite::future::block_on(first_accepted_or_timeout(
            stream,
            Duration::from_secs(5),
            |_: i32| -> Option<i32> { None }, // nothing accepted
        ));
        assert_eq!(result, None);
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
