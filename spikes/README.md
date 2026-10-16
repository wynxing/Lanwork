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

应用页的界面预览在 `spikes/apps-page-preview`，包名 `lanwork-apps-preview`。不进 `lanwork` 的依赖，不进安装包，也不进产品界面。它只画出静态状态，不安装工具，不读数据目录。

```
cargo run -p lanwork-apps-preview -- --out shots
```

工具的宿主在 `crates/toolhost`，包名 `lanwork-toolhost`。安全测试在这里。窗口验证仍在 `spikes/toolhost`，包名 `lanwork-toolhost-spike`，依赖产品 crate，不进安装包。它读开发说明里的额度示例，不写 `%USERPROFILE%\Documents\Lanwork`，也不读 MayDolist。`run` 开一扇软件渲染的悬浮窗。`measure` 在无桌面的软件窗口上记一帧，再让进程睡一会儿。2026-10-15 的 Windows 11 手测见 [docs/roadmap.md](../docs/roadmap.md)「技术验证」。凭据管理器界面待用户用 `lanwork --toolhost-cred write|list|clear` 确认。内存和 CPU 的数字上限仍待定。

```
cargo test -p lanwork-toolhost-spike
cargo run -p lanwork-toolhost-spike -- run --example quota
cargo run -p lanwork-toolhost-spike --release -- measure --example quota --secs 5 --out measure.json
```

采样用 `lanwork-sample`，按上面打印的 `pid`。进程名是 `hotcorner.exe`，计数器是 `\Thread(hotcorner*)\Context Switches/sec`。

```
cargo run -p lanwork-sample --release -- sample --pid <pid> --duration-secs 300 --interval-ms 1000 --out sample.csv
```
