cargo run -p hello
cargo run -p fileidx -- probe

渲染器和背景的测量程序是 `lanwork-render-spike`。四个 feature 互斥，默认是 FemtoVG。换渲染器时要关掉默认 feature：

```text
cargo run -p lanwork-render-spike --no-default-features --features femtovg -- --scene a
cargo run -p lanwork-render-spike --no-default-features --features skia-software -- --scene b --animate
cargo run -p lanwork-render-spike --no-default-features --features skia-opengl -- --scene a
cargo run -p lanwork-render-spike --no-default-features --features software -- --scene a
```

`--scene a` 是空面板，`--scene b` 是 200 行列表。`--shot-dir` 写截图，`--status-out` 写 JSON。`--exit-after-secs` 到时退出。`--self-test-black` 先铺一块不透明黑，用来看检测能不能发现，再退回纯色；这不是渲染器自己画黑。`--theme dark` 只改这个窗口的暗色属性和纯色，不改系统主题。`--reference` 在窗口紧后面放一块品红，方便看透不透；内存采样不要加它。程序只读「透明效果」和节电状态，不改系统设置。
