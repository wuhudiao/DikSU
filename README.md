# DikSU

DikSU 是基于 [KernelSU](https://github.com/tiann/KernelSU) 的 Android 内核级 Root 方案分支，在原版能力之上提供了更完整的界面定制、伪装入口与网页端管理能力。

## 特性

- **三档界面风格**：`miuix(美化版)` / `miuix(原版)` / `Material 3`，在设置中自由切换；原版与 MD3 与官方初始布局一致
- **美化版 UI**：图片 / 视频壁纸背景、玻璃拟态导航与面板、可调背景暗度
- **计算器启动器**：伪装计算器作为网页端入口，输入触发码按 `=` 即可启动服务，不用时端口不监听
- **网页端管理**：ksud 内置 HTTP 服务，应用授权、模块管理、文件操作一屏搞定，应用图标与名称由内置 dex 实时读取
- **隐藏能力**：一键隐藏管理器（重打包换包名）+ 网页端隐藏开关，开启后首页显示「未安装」、底部导航同步消失

## 构建

```bash
# ksud（Linux / macOS / WSL）
cargo build --release --target aarch64-linux-android --manifest-path ./userspace/ksud/Cargo.toml

# Manager（需要先放置 libksud.so 到 app/src/main/jniLibs/arm64-v8a/）
cd manager && ./gradlew assembleRelease

# 内核模块（LKM）
cd kernel && ./build-all.sh
```

## 致谢

- 感谢 [Aster](https://github.com/LyraVoid/Aster) 贡献的美化版 UI 设计与实现
- 感谢 [KernelSU](https://github.com/tiann/KernelSU) 原项目及社区

## 社区

- GitHub：https://github.com/wuhudiao/DikSU
- Telegram：https://t.me/DIKSU66
- QQ 群：https://qun.qq.com/universal-share/share?ac=1&authKey=74nnnyTyqUBAScf9f4COtRw9M2X6X%2BueD2nj3wirB3TcABLk%2BVnyA3sAQ2%2BJA7VB&busi_data=eyJncm91cENvZGUiOiI4NjQ1NTMzNjciLCJ0b2tlbiI6IjhQQ0Q4Y2hQamRjTUI5VEN4WlV5aXYxUjBHWi8waEh6RHh6bVlwM3VrMWo5RVI1aHM5Wlg5a1BKSFU0ZkRrR0wiLCJ1aW4iOiIyODQ3NzM4MjExIn0%3D&data=PLE8cWnp_e0Nnj02328MXN8lMA7I2kndDWxdhCLYQ5UrmBpOZWEdkbFKyyA9Kp2UtUtunM0jd6Dr5whKMHU0AA&svctype=4&tempid=h5_group_info

## License

[GPL-3.0](LICENSE)，与 KernelSU 相同。
