# -*- coding: utf-8 -*-
"""Generate the M11 docs apply_patch envelope and self-verify it applies cleanly."""
import difflib
import os

WS = "/home/acacia/touchpad"
SCRATCH = os.path.join(WS, ".patchwork")

def load(rel):
    with open(os.path.join(WS, rel), encoding="utf-8") as f:
        return f.read()

def replace_once(text, old, new, where):
    n = text.count(old)
    assert n == 1, f"{where}: expected exactly 1 occurrence, found {n}"
    return text.replace(old, new)

# ------------------------------------------------------------------ README.md
readme = load("README.md")

readme = replace_once(readme, """Re-review 2). Status of M10: **code approved (M10_REVIEW.md Re-review 1),
live-unqualified / pending user acceptance** — the
static/fake-backed gates pass, but the 10/60/300-second takeover sequence
and the M6 output calibration must be recorded by the user before any live
qualification (`docs/M10_ACCEPTANCE.md`; `--output-qualified` is an operator
attestation, not measurement evidence).
M1–M6 are approved.""", """Re-review 2). Status of M10: **code approved (M10_REVIEW.md Re-review 1),
live-unqualified / pending user acceptance** — the
static/fake-backed gates pass, but the M6 output calibration and the ordered
10-second, 60-second, then 300-second takeover sequence must be recorded by
the user before any live qualification (`docs/M10_ACCEPTANCE.md`;
`--output-qualified` is an operator attestation, not measurement evidence).
Status of M11: **implemented, under independent re-review (not yet
review-approved); experimental, opt-in `m11-fidelity-v1`, never the default,
no macOS equivalence claim; live-unqualified** — M11 code completion does not
imply live qualification, and M11 stays live-unqualified until the separate,
later M11-specific user acceptance (`docs/M11_ACCEPTANCE.md`, written, not
executed) is passed. M10 acceptance does not qualify M11. M12 has not begun.
M1–M6 are approved.""", "README status paragraph")

readme = replace_once(readme,
    "**M10: `m10-linear-v1` takeover profile** |",
    "**M10: `m10-linear-v1` takeover profile**; **M11: experimental `m11-fidelity-v1` one-finger pointer fidelity (opt-in, never default, no macOS equivalence claim, live-unqualified)** |",
    "README workspace core row")

readme = replace_once(readme,
    "**`takeover` (M10, implemented, R1–R6 repaired, pending re-review, live-unqualified)** |",
    "**`takeover` (M10 code approved, live-unqualified; M11 adds the accepted `--profile` value `m11-fidelity-v1` — experimental, live-unqualified)** |",
    "README workspace touchpadctl row")

readme = replace_once(readme, "## Build and test\n\nStable Rust.",
"""## M11: experimental one-finger pointer fidelity (implemented, under review)

M11 layers an **experimental, opt-in, never-default** one-finger
pointer-fidelity stage on the approved M7–M9 interaction policy, exposed only
as the `m11-fidelity-v1` value of the existing mandatory `--profile` option
(the accepted set is exactly `{m10-linear-v1, m11-fidelity-v1}`). It makes
**no macOS equivalence claim**. The pure, platform-independent stage adds a
signed radial dead zone, a monotonic time-domain velocity estimate, a bounded
smoothstep gain curve, and an explicit tracking multiplier for committed
one-finger millimeter motion; `M11Profile` inherits every M7–M9 value from
`m10-linear-v1` without copying constants. Fidelity state lives in the
Arbiter's atomic draft (a rejected frame rolls it back), the fidelity-disabled
M10 path is unchanged, and the CLI prints the experimental banner before any
device/output/recorder/countdown/grab side effect. Status: **implemented,
under independent re-review (not yet review-approved), live-unqualified** —
code completion does not imply live qualification; M11 stays live-unqualified
until the separate, later user acceptance in `docs/M11_ACCEPTANCE.md`
(written, not executed) is passed. M10 acceptance does not qualify M11. M12
has not begun.

## Build and test

Stable Rust.""", "README M11 section insert")

readme = replace_once(readme, "**792 tests**", "**872 tests**", "README test count")

readme = replace_once(readme, """and `docs/M10_ACCEPTANCE.md` (M10, the user-run 10/60/300-second takeover
sequence and the M6 output-calibration table that must be filled before
honestly passing `--output-qualified`).""", """`docs/M10_ACCEPTANCE.md` (M10, the user-run 10/60/300-second takeover
sequence and the M6 output-calibration table that must be filled before
honestly passing `--output-qualified`), and `docs/M11_ACCEPTANCE.md` (M11,
the future, **not-yet-executed** user-run acceptance for the experimental
`m11-fidelity-v1` profile; M10 acceptance does not qualify M11).""",
    "README acceptance docs")

# ---------------------------------------------------------------- DESIGN_V2.md
design = load("DESIGN_V2.md")

design = replace_once(design,
    "（M9_REVIEW.md Re-review 2，2026-08-17，见 §20；M9_TASK.md 为绑定范围与验收契约）。",
    "（M9_REVIEW.md Re-review 2，2026-08-17，见 §20；M9_TASK.md 为绑定范围与验收契约）；**M10（限时安全 Takeover 纵向切片）代码已批准、live-unqualified**（M10_REVIEW.md Re-review 1，2026-08-17，见 §21）；**M11（实验性一指指针保真 `m11-fidelity-v1`）已实现、独立 re-review 中、未批准、live-unqualified**（见 §22；M11_REVIEW.md 2026-08-22 的 R1–R4 修复完成，待复审；不宣称 review 通过或 live 合格）。",
    "DESIGN header status")

design = replace_once(design, "并记录 M6 输出校准表。M11 未开始。",
    "并记录 M6 输出校准表。M11 见 §22（已实现，独立 re-review 中，未批准）。",
    "DESIGN 21 status")

design = replace_once(design, "M10 不写 approved，M11 未开始。",
    "M10 不写 approved；M11 见 §22（已实现，未批准，live-unqualified）。",
    "DESIGN 21.9")

section22 = """## 22. M11 — 实验性一指指针保真（`m11-fidelity-v1`）（已实现，独立 re-review 中，live-unqualified）

状态：**已实现（2026-08-22）**。`M11_REVIEW.md`（2026-08-22）的 R1–R4 阻塞项已修复：R1（M11 trace/replay fixture 与直接-重放决策一致性测试）、R2（`select_profile`/banner 与 fake-backed CLI 测试）、R3（fmt/clippy 门禁）、R4（`M11Profile::new` 以 `M11ProfileError::Fidelity` 传播而非 panic）。独立 re-review **尚未通过**——本文件不宣称 M11 review 通过、不宣称 code-complete。**M11 保持 live-unqualified**，直到用户按 `docs/M11_ACCEPTANCE.md`（未来人工验收程序，**写而未执行**）完成独立的 M11 专用验收；M10 验收不使 M11 合格。M12 未开始。

### 22.1 范围与边界

M11 在已批准的 M7–M9/M10 交互策略之上增加**平台无关的一指指针保真阶段**，仅作用于已提交的一指毫米位移（原始 counts 永不进入；candidate/tap/scroll 归属仍基于保真前的毫米位置）。`m11-fidelity-v1` 是既有强制 `--profile` 选项的第二个接受值——接受集恰为 `{m10-linear-v1, m11-fidelity-v1}`，`m10-linear-v1` 保持 mention-first；实验性、opt-in、永不默认、**无 macOS 等价声明**。M10 的全部五个 opt-in（`--takeover`、`--confirm TAKEOVER`、`--output-qualified`、`--profile`、`--max-duration-seconds N`）与 `1..=300` 秒上界原样保留；M11 不新增 flag。

### 22.2 实现事实（`M11_TASK.md` §5–§11）

- `touchpad-core::fidelity`：`FidelityConfig`/`FidelityState`/`FidelityOutcome`（`Hold`/`EmitScaledPixels`/`Reanchored`）/`FidelityError` 与纯 `process` 阶段——signed radial dead-zone（0.09 mm）、单调时域速度估计（EMA，20 ms tau）、连续有界 smoothstep gain（50–600 mm/s → 1.0–2.0）、显式 tracking 倍率（1.0）、base 10 px/mm（继承 M10）；`FidelityConfigError` 覆盖非法构造，运行期非有限/溢出为 `FidelityError`，Arbiter fail-closed 映射为 `ArbiterError::NonFinite`。
- `touchpad-core::m11`：`M11Profile` 经 `M10Profile::new()?.arbiter_config()` 继承全部 M7–M9 值并 `.with_fidelity(...)`——不复制常量、不改 `m10.rs`；R4 修复使构造错误经 `M11ProfileError::Fidelity` 传播而非 panic。
- `arbiter` 集成：`ArbiterConfig` 增 `Option<FidelityConfig>`（默认 `None`）+ `with_fidelity`/`fidelity_config`/`is_fidelity_enabled`；`FidelityState` 存入 `ArbiterState`，参与既有 draft/copy/commit 原子性（拒绝帧回滚全部保真状态与 pixel remainder，无第二 remainder）；一个窄 `emit_pointer_delta` 模式开关——fidelity 关闭时执行既有 M10 量化分支且不调用保真代码；`Hold`/`Reanchored` 零输出并保留 remainder；clean end/replacement/discontinuity/cancellation/`release_all` 的 reset 顺序符合 `M11_TASK.md` §9。
- `touchpadctl`：`args.rs` 接受集恰为 `{m10-linear-v1, m11-fidelity-v1}` 且 M10 mention-first、`--profile` 缺失不推断；`cmd::takeover` 引入纯 `select_profile`（M10 → 恰 M10 config、M11 → 恰 M11 config）+ `M11_EXPERIMENTAL_BANNER`（experimental/uncalibrated、非默认、无 macOS 等价声明、无 live 验证、M10 opt-in 与 1..=300 秒上界仍适用），banner 在任何 device/output/recorder/countdown/grab 副作用之前写出；fake-backed 命令/CLI 测试保留全部五个 opt-in、重复拒绝、缺 profile 与时长边界，不进入真实 takeover。
- trace/replay：`crates/touchpad-trace/tests/fixtures/m11_fidelity.jsonl`（25 帧确定性场景：first commit、低/高速、重复时间戳、reversal、对角、恰等于/超过 `long_gap`、clean end、fresh interaction；`fixtures.rs`/`replay.rs` 注册）；`crates/touchpad-linux/tests/m11_arbiter.rs` 证明直接合成帧与 replay 派生帧产生**相同** M11 决策。
- `docs/M11_ACCEPTANCE.md`：未来用户人工验收程序（**写而未执行**）；要求 M10/M6 前置（M10 代码批准 + M6 校准记录 + 10/60/300 秒 M10 验收），分离 M11 code-complete 与 live-qualified。

### 22.3 测试与门禁

- `crates/touchpad-core/tests/m11_fidelity.rs`（57 个测试）：配置边界、首调用全位移 min_gain 保留且不进下一速度分子、重复时间戳折叠不 flush、零 dt 不除不造速、long-gap 边界（−1 ns / 恰好 / 超过）、符号取消/reversal/对角、smoothstep 连续单调有界、60/120 Hz 相对差 ≤1%、tracking 各向同性、remainder 不变量/无 epsilon drain/拒绝帧回滚、clean end/replacement/discontinuity/cancellation/`release_all`、tap/tap-drag/drag-lock/双指/物理键、M10 关闭回归、profile 继承。
- `apps/touchpadctl/src/cmd/takeover/tests.rs`：`select_profile` 纯路由（M10 恰 M10 config 且 fidelity 关闭、M11 恰 M11 config 且 fidelity 开启）、未知 profile 无副作用失败、M11 banner 五条声明；`apps/touchpadctl/tests/cli.rs`：fake-backed `m11-fidelity-v1` 公共 CLI 全 opt-in 保留、banner 先于副作用、决策经 fake 输出。
- 门禁（Part 5 终验实测）：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings`、`cargo test --workspace --locked`、`cargo test --release --workspace --locked` 全部通过；workspace 共 **872** 个测试（debug 与 release 各 872，0 失败；M10 792 + M11 新增 80，含 core `m11_fidelity` 57、linux `m11_arbiter` 6、`M11Profile` 单元与 `select_profile`/banner/fake-backed CLI 用例）。
- 无新依赖、无新 `unsafe`（`touchpad-core` 保持 `#![forbid(unsafe_code)]`）、`m10.rs` 未修改；无 live 命令/真实设备/portal/libei 副作用；无凭据写入变更文本。
"""
design = design.rstrip("\n") + "\n\n" + section22

# ---------------------------------------------------------------- MILESTONES.md
milestones = load("MILESTONES.md")

milestones = replace_once(milestones,
    "M10 保持 live-unqualified；M11 未开始。实现事实见 `DESIGN_V2.md` §21。",
    "M10 保持 live-unqualified；M11 已按离线约束实施（见下条）。实现事实见 `DESIGN_V2.md` §21。",
    "MILESTONES M10 entry tail")

milestones = replace_once(milestones,
    "**M10 状态更新（2026-08-17）**：`reviews/M10_REVIEW.md` Re-review 1 已批准 M10 代码，R1–R6 全部关闭；仍为 `live-unqualified / pending user acceptance`，在用户完成 M6 校准记录与 10/60/300 秒真机序列前不得进入 M11。",
    "**M10 状态更新（2026-08-17）**：`reviews/M10_REVIEW.md` Re-review 1 已批准 M10 代码，R1–R6 全部关闭；仍为 `live-unqualified / pending user acceptance`——用户完成 M6 校准记录与 10/60/300 秒真机序列前不得宣称 M10 live 合格。M11 已按离线约束实施（见下条），其 live 验收独立于 M10。",
    "MILESTONES M10 status update")

m11_milestone = """## M11 — 实验性一指指针保真（`m11-fidelity-v1`）（在 PHASE2_PLAN.md §5 定义）

**(已实现，2026-08-22；`M11_REVIEW.md` R1–R4 修复完成；独立 re-review 中——未批准、未 code-complete；live-unqualified / 待独立的 M11 专用用户验收——代码完成不意味着 live 合格，M10 验收不使 M11 合格，M12 未开始）**：在已批准的 M7–M9/M10 交互策略之上增加平台无关的一指指针保真阶段（signed radial dead-zone 0.09 mm、20 ms tau 单调时域速度 EMA、150 ms 含边界 long-gap、50–600 mm/s → gain 1.0–2.0 smoothstep、tracking 倍率 1.0、base 10 px/mm 继承 M10）。`M11Profile` 从 `M10Profile` 继承全部 M7–M9 值（不复制常量、不改 `m10.rs`）；`FidelityState` 存入 `ArbiterState` 参与既有 draft/commit 原子性（拒绝帧回滚），fidelity 关闭时 M10 分支不变；`touchpadctl --profile` 接受集恰为 `{m10-linear-v1, m11-fidelity-v1}`（M10 mention-first），纯 `select_profile` + 实验 banner（experimental/uncalibrated、非默认、无 macOS 等价声明、无 live 验证、M10 五个 opt-in 与 1..=300 秒上界仍适用）在任何副作用之前写出；确定性 trace fixture `m11_fidelity.jsonl`（25 帧：first commit/低高速/重复时间戳/reversal/对角/恰等于与超过 long-gap/clean end/fresh interaction）+ 直接-重放决策一致性测试；`docs/M11_ACCEPTANCE.md` 为未来用户人工验收程序（写而未执行；要求 M10/M6 前置，分离 M11 code-complete 与 live-qualified）。实现事实见 `DESIGN_V2.md` §22；workspace 共 **872** 个测试（debug/release 门禁实测，M10 792 + M11 新增 80，0 失败）。
"""
milestones = replace_once(milestones, "## Review 输出格式",
    m11_milestone + "## Review 输出格式", "MILESTONES M11 section insert")

# ---------------------------------------------------------------- expected files
expected = {
    "README.md": readme,
    "DESIGN_V2.md": design,
    "MILESTONES.md": milestones,
}
with open(os.path.join(SCRATCH, "m11_acceptance.md"), encoding="utf-8") as f:
    expected["docs/M11_ACCEPTANCE.md"] = f.read()

for rel, text in expected.items():
    path = os.path.join(SCRATCH, "expected", rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)

# ---------------------------------------------------------------- patch generation
def patch_for(rel, old_text, new_text):
    diff = difflib.unified_diff(old_text.splitlines(keepends=True),
                                new_text.splitlines(keepends=True), n=2)
    out = [f"*** Update File: {rel}\n"]
    hunk = []
    in_hunk = False
    for line in diff:
        if line.startswith(("--- ", "+++ ")):
            continue
        if line.startswith("@@ "):
            if in_hunk:
                out.append("@@\n")
                out.extend(hunk)
            hunk = []
            in_hunk = True
            continue
        hunk.append(line)
    if in_hunk:
        out.append("@@\n")
        out.extend(hunk)
    return "".join(out)

patch = ["*** Begin Patch\n"]
patch.append("*** Add File: docs/M11_ACCEPTANCE.md\n")
for line in expected["docs/M11_ACCEPTANCE.md"].splitlines(keepends=True):
    patch.append("+" + line)
for rel in ["README.md", "DESIGN_V2.md", "MILESTONES.md"]:
    patch.append(patch_for(rel, load(rel), expected[rel]))
patch.append("*** End Patch\n")
patch_text = "".join(patch)

with open(os.path.join(SCRATCH, "m11_docs.patch"), "w", encoding="utf-8") as f:
    f.write(patch_text)

# ---------------------------------------------------------------- verify: apply to pristine copies
def find_hunk_start(text_lines, seq, cursor):
    if not seq:
        return cursor
    def matches(pos):
        if pos + len(seq) > len(text_lines):
            return False
        for (typ, t), actual in zip(seq, text_lines[pos:pos + len(seq)]):
            if typ == "+":
                continue
            if actual.rstrip("\n") != t.rstrip("\n"):
                return False
        return True
    pos = cursor
    while pos <= len(text_lines):
        if matches(pos):
            return pos
        pos += 1
    raise AssertionError("hunk not found in " + rel)

files = {rel: load(rel) for rel in ["README.md", "DESIGN_V2.md", "MILESTONES.md"]}
lines = patch_text.splitlines(keepends=True)
i = 0
assert lines[0].strip() == "*** Begin Patch"
i = 1
while i < len(lines):
    line = lines[i]
    if line.strip() == "*** End Patch":
        break
    assert line.startswith("*** "), f"expected section header, got {line!r}"
    header = line.strip()[4:]
    assert header.startswith(("Add File:", "Update File:")), header
    kind, rel = header.split(":", 1)
    rel = rel.strip()
    i += 1
    if kind == "Add File":
        content = []
        while i < len(lines) and not lines[i].startswith("*** "):
            assert lines[i].startswith("+"), lines[i]
            content.append(lines[i][1:])
            i += 1
        files[rel] = "".join(content)
    else:
        text_lines = files[rel].splitlines(keepends=True)
        cursor = 0
        while i < len(lines) and not lines[i].startswith("*** "):
            assert lines[i].strip() == "@@", f"expected @@, got {lines[i]!r}"
            i += 1
            body = []
            while i < len(lines) and not lines[i].startswith("*** ") and lines[i].strip() != "@@":
                body.append(lines[i])
                i += 1
            tokens = []
            for b in body:
                if b.startswith(" "):
                    tokens.append((" ", b[1:]))
                elif b.startswith("-"):
                    tokens.append(("-", b[1:]))
                elif b.startswith("+"):
                    tokens.append(("+", b[1:]))
                else:
                    raise ValueError(f"bad hunk line {b!r}")
            seq = [(typ, t) for typ, t in tokens if typ != "+"]
            start = find_hunk_start(text_lines, seq, cursor)
            new_segment = []
            pos = start
            for typ, t in tokens:
                if typ in (" ", "-"):
                    assert text_lines[pos].rstrip("\n") == t.rstrip("\n"), \
                        (rel, text_lines[pos], t)
                    if typ == " ":
                        new_segment.append(text_lines[pos])
                    pos += 1
                else:
                    new_segment.append(t if t.endswith("\n") else t + "\n")
            text_lines = text_lines[:start] + new_segment + text_lines[pos:]
            cursor = start + len(new_segment)
        files[rel] = "".join(text_lines)

ok = True
for rel, want in expected.items():
    got = files.get(rel)
    if got != want:
        ok = False
        print(f"MISMATCH: {rel}")
        import difflib as d
        for dl in d.unified_diff(want.splitlines(True), got.splitlines(True), "expected", "applied"):
            print(dl, end="")
print("VERIFY:", "PASS — patch applies cleanly and reproduces expected files" if ok else "FAIL")
