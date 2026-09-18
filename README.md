# DikSU

基于 [KernelSU](https://github.com/tiann/KernelSU) 的个人分支（fork）：一个内核级的 Android
root 方案，外加一个在自己设备上使用的网页管理器。

> **这是源码快照。** 仓库保留管理器、网页端与内核模块的源码；与个人设备相关的部分未包含在内，
> 因此**不能直接编译**，也不面向使用或分发。想装能用的一版，请使用上游
> [KernelSU](https://github.com/tiann/KernelSU)。

## 这个分支有什么不一样

- **网页管理器**：管理器不再只有一个 App。root 之后可以在手机浏览器里管理设备；页面是单文件、
  随管理器一起分发，界面按 Material 3 与液态玻璃两套风格做过适配，跟随系统明暗主题。
- **文件管理器**：双栏（左栏选中、右栏当目的地），支持复制 / 移动 / 删除 / 重命名 / 新建文件夹；
  可查看与修改权限（八进制输入 + `777/755/644/600` 预设）和属主；支持长按滑动批量拖选、
  压缩包内浏览与解压、文本编辑、图片预览、上传下载。
- **终端**：跑在设备上的持久 `sh` 会话，支持交互式输入，输出按纯文本渲染。
- **Oh My Keymint 面板**：给 [Oh My Keymint](https://github.com/qwq233/OhMyKeymint) 这类模块用的
  控制面板 —— 读写它的配置、切换 keybox、重启 keymint 与 injector；写之前会校验 TOML，配置写坏
  就不保存（写坏的 `config.toml` 会让 keymint 起不来）。
- **应用与模块**：查看已安装应用、按 UID 授权/撤销 root、查看 App Profile；模块的安装、卸载，
  以及模块自带 WebUI 的加载都在这里完成。
- **一键配置隐藏应用列表**：把第三方应用加入隐藏范围，并顺手把自己从其他应用里隐藏掉；
  另有一个变体照顾需要无障碍读应用的工具。

## 与上游的关系

这个分支只动了两件事：**内核侧允许的管理器签名**（换成本分支自己的证书），以及上面这些用户态
功能。内核侧的超调用（supercall）、allowlist、App Profile、模块系统这些核心机制都来自上游，
语义没有被改动。

感谢上游作者与社区：

- **[KernelSU](https://github.com/tiann/KernelSU)**（tiann 与所有贡献者）—— 本分支的基础；
  内核侧的 root 与模块系统都是他们的工作。
- **[Kernel-Assisted Superuser](https://git.zx2c4.com/kernel-assisted-superuser/about/)** ——
  KernelSU 最初的思路来源。
- **[Magisk](https://github.com/topjohnwu/Magisk)**（topjohnwu）—— 现代 Android root 方案的
  先行者。
- **[Oh My Keymint](https://github.com/qwq233/OhMyKeymint)** —— KeyMint 面板所服务的模块。

## 许可证

- `kernel/` 目录：**GPL-2.0-only**
- 其余部分：**GPL-3.0-or-later**

与上游保持一致；使用时请遵守各自的许可证。
