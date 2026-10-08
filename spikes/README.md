cargo run -p hello
cargo run -p ime
cargo run -p fileidx -- probe

渲染器和背景的测量程序是 `lanwork-render-spike`。四个 feature 互斥，默认是 FemtoVG。换渲染器时要关掉默认 feature：

```text
cargo run -p lanwork-render-spike --no-default-features --features femtovg -- --scene a
cargo run -p lanwork-render-spike --no-default-features --features skia-software -- --scene b --animate
cargo run -p lanwork-render-spike --no-default-features --features skia-opengl -- --scene a
cargo run -p lanwork-render-spike --no-default-features --features software -- --scene a
```

`--scene a` 是空面板，`--scene b` 是 200 行列表。`--shot-dir` 写截图，`--status-out` 写 JSON。`--exit-after-secs` 到时退出。`--self-test-black` 先铺一块不透明黑，用来看检测能不能发现，再退回纯色；这不是渲染器自己画黑。`--theme dark` 只改这个窗口的暗色属性和纯色，不改系统主题。`--reference` 在窗口紧后面放一块品红，方便看透不透；内存采样不要加它。程序只读「透明效果」和节电状态，不改系统设置。

热角技术验证在 `spikes/hotcorner`，不进 `lanwork` 的依赖。`--corner-px` 和 `--monitor` 是待定参数，产品规格没有写。`--dwell-ms` 的默认值 350 来自产品规格。进程到 `--duration-secs` 后退出；方案 A 会卸下钩子。

```
cargo run -p hotcorner -- probe --out probe.json
cargo run -p hotcorner -- self-test --out self-test.json --corner-px 32
cargo run -p hotcorner --release -- run --scheme hook --monitor primary --corner top-right --corner-px 32 --dwell-ms 350 --duration-secs 360 --events events.jsonl
cargo run -p hotcorner --release -- run --scheme poll --interval-ms 50 --monitor primary --corner top-right --corner-px 32 --dwell-ms 350 --duration-secs 360
cargo run -p hotcorner --release -- run --scheme poll --interval-ms 100 --monitor primary --corner top-right --corner-px 32 --dwell-ms 350 --duration-secs 360
```

采样用 `lanwork-sample`，按上面打印的 `pid`。进程名是 `hotcorner.exe`，计数器是 `\Thread(hotcorner*)\Context Switches/sec`。

```
cargo run -p lanwork-sample --release -- sample --pid <pid> --duration-secs 300 --interval-ms 1000 --out sample.csv
```

cargo run -p toast --release -- help
cargo run -p toast --release -- register installed
cargo run -p toast --release -- register portable
cargo run -p toast --release -- unregister
cargo run -p toast --release -- status
cargo run -p toast --release -- serve
cargo run -p toast --release -- show todo-001
cargo run -p toast --release -- show todo-001 --repeat 3
cargo run -p toast --release -- clear-history
cargo run -p toast --release -- activation-check
cargo run -p toast --release -- self-check

`activation-check` 不点击通知，也不写 HKCU。它在本进程创建模拟面板，再用另一个进程 `CoCreateInstance` 激活器并调用 `Activate`。本机已经有 `toast.exe` 时，它不注册产品 CLSID，只注册一个内存里的探测 CLSID，避免抢走正在运行的服务器。`self-check` 会先做这一步，再做注册表往返；结束时仍会注销 spike 的 HKCU 项。
