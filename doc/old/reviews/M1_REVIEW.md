# M1 External Review

状态：**Approved after revision**。M1 已批准，可以进入 M2。

## 修订后复核

R1–R3 已由执行代理修复，并由外部 reviewer 再次逐文件检查：

- 绝对 position 与相对 delta 已拆成独立 API；position 使用 `i64` 中间值计算 `(raw - min) / resolution`，profile override 保留原点。
- `Monotonic::now()` 已移除；core 不再读取时钟。
- panicking constructors 和算术 operator traits 已移除；单位构造返回 `Result`，算术使用 checked API。
- 未越界实现 M2+，未发现凭据落盘。

外部 reviewer 独立复跑：

```text
cargo fmt --check                                      PASS
cargo clippy --workspace --all-targets -- -D warnings  PASS
cargo test --workspace                                 PASS（36 unit + 8 integration）
```

## 已验证

- `cargo fmt --check`：通过。
- `cargo clippy --workspace --all-targets -- -D warnings`：通过。
- `cargo test --workspace`：通过，32 个单元测试 + 6 个集成测试，共 38 个。
- workspace 仅包含 `touchpad-core`，未越界实施 M2+。
- 未发现凭据写入仓库。

## 必须修正

### R1 — 绝对坐标转换忽略轴原点

当前 `raw_axis_to_mm(raw, info)` 只计算 `raw / resolution`，没有使用 `AxisInfo.min`。对 `min != 0` 的绝对轴，这会让触控板坐标整体偏移，且设备物理宽高无法稳定从 range/resolution 推导。

要求：

- 明确区分绝对 position conversion 与相对 delta conversion，避免一个 API 同时承担两种不兼容语义。
- 绝对坐标应将轴最小值映射到 `0 mm`，即使用等价于 `(raw - min) / resolution` 的安全计算；中间计算不得发生 i32 overflow。
- profile resolution override 必须保持相同原点语义。
- 添加 `min != 0`、边界值和相对 delta 的测试，并在 `DESIGN_V2.md` 记录坐标原点决策。

### R2 — `Monotonic` 混入不同 clock domain

`Monotonic::now()` 生成“进程启动后相对时间”，而类型文档同时允许 kernel evdev/`CLOCK_MONOTONIC` 的时间戳。二者都包装为相同 `u64`，可以直接比较，结果在类型上合法但语义错误。后续 replay 还会增加 trace clock domain。

要求：

- core 只表示由平台输入层/trace 提供的单调时间，不在 core 内自行读取另一个 clock；优先移除 `Monotonic::now()`。
- 如果保留多个时间源，必须用不可混用的 domain 类型显式建模，而不是仅靠注释。
- 更新测试和 `DESIGN_V2.md`，说明 live/replay 时间戳由边界层提供且同一 interaction 只能使用一个 domain。

### R3 — 单位 API 的 panic 路径违反 fail-open

`Millimeters::new`、`LogicalPixels::new` 使用 `expect`；`Add`、`Sub`、`Neg` 和 assign traits 又调用这些构造函数。运行时输入或算法溢出可因此 panic。触控板被 grab 后，panic 不能成为正常错误处理策略。

要求：

- 外部数据和运行时计算必须使用显式、结构化的 fallible API；不得依靠 `expect`/panic 保持 finite 不变量。
- 删除或重设可能 panic 的算术 trait；保留清晰的 checked 运算。
- 构造/反序列化/运算仍必须保证 `NaN`/Infinity 无法进入有效值。
- 测试必须验证错误返回而不是 `catch_unwind`。
- 同步修正 `DESIGN_V2.md` 中把 panic constructor 描述为已批准设计的内容。

## 修订验收

```text
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

修订只能处理 M1 和本 review，不得开始 trace、decoder、ioctl、grab 或 CLI。
