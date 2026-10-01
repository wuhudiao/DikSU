**English** | [Español](README_ES.md) | [简体中文](README_CN.md) | [繁體中文](README_TW.md) | [日本語](README_JP.md) | [한국어](README_KR.md) | [Polski](README_PL.md) | [Português (Brasil)](README_PT-BR.md) | [Türkçe](README_TR.md) | [Русский](README_RU.md) | [Tiếng Việt](README_VI.md) | [Indonesia](README_ID.md) | [עברית](README_IW.md) | [हिंदी](README_IN.md) | [Italiano](README_IT.md)

# DikSU

- GitHub: https://github.com/wuhudiao/DikSU
- Telegram: https://t.me/DIKSU66
- QQ group: `864553367`

---

## 1. WebUI: a complete management interface the original does not have

The original KernelSU WebUI only hosts the modules' own WebUI pages. DikSU builds an HTTP service directly into `ksud`, putting the whole management interface into the browser, so most operations can be done without the Manager app.

All the code lives in `userspace/ksud/src/webui*.rs` and `userspace/ksud/assets/web/`: about 4900 lines of front end and 8900 lines of server, exposing 60+ `/api/*` endpoints.

### 1. Home and status
- System info on one screen: kernel version, device model, system fingerprint, SELinux status (Enforcing highlighted), Seccomp status (filtered / strict / disabled / unsupported)
- Feature switches toggled right in the page, with kernel capability detection; unsupported items are greyed out
- Built-in update check pointing at DikSU's own Releases (the original points at KernelSU)
- One tap to join the QQ group

### 2. Superuser
- App list, search by name / package name
- Grant, revoke, and view an individual app's grant details

### 3. Module management
- Online install / local install, with a live progress bar, install log output, and **cancel midway**
- Module list, module info, open a module's own WebUI
- Read package names and icons of apps inside a module

### 4. File manager (dual pane)
- Browse in two panes; copy / move to the opposite side
- Permission and owner changes (`chmod` / `chown`, recursive, root)
- New folder, new file, rename, delete, search
- Archives: extract to the opposite side, add to an archive, create an archive, and **browse inside the archive** to add, delete and modify
- Upload, download, text editor (with line numbers)
- "Open with", run a script, add to quick jumps and quick actions
- Install a file directly as a module

### 5. Terminal
- PTY-based interactive shell, up to 4 concurrent sessions, 256 KB output buffer
- Command history, live output stream

### 6. Background jobs (spawn)
- Run 8 background commands concurrently, with stdout / stderr polled separately
- Jobs can be closed at any time; unread output capped at 8 MB

### 7. Apps and APK parsing
- Full app list, query a single app's details, extract the APK
- `apkparser.rs`: parses the binary `AndroidManifest.xml` and `resources.arsc` directly **without the Android framework** to get app labels, together with `appicon.dex` / `applabel.dex` for icons. This lets the WebUI show app names and icons even in a pure shell environment

### 8. keyMint page
- Status of the keymint / injector daemons, with individual or one-shot restart
- System property fix switch (writes `/data/adb/service.d/omk-fixprops.sh`, putting the unlocked / debuggable properties back to a normal device's on every boot)
- App routing: choose which apps go through Oh My Keymint; changes apply to new requests at once, no reboot needed
- keybox management: pick a `keybox.xml` already on the device, or fill in a URL to download and replace it; it only takes effect after it checks out
- Log levels are written to `config.toml` and `injector.toml` respectively

### 9. One-tap Hide My Applist config
- Built-in HMA-OSS config script: adds every third-party app that has no entry yet to the hidden scope in one go. Apps already configured only get their presets filled in, other fields untouched, and the Manager is hidden from all of them too
- A **Standard** variant and a **Scene** variant (the Scene variant additionally keeps Scene itself out of the scope and leaves the accessibility preset unchecked)

### 10. Entry point: a calculator
The WebUI is not opened from the Manager; it hides inside a calculator:

- A bundled `calculator.apk` that looks like an ordinary third-party calculator once installed
- Type the code into `/data/adb/ksu/webui.trigger` and press `=` to start the service and open the page
- The code and package name live in `/data/adb/ksu/calculator.pkg`, and the package name differs on every install; nothing on the device can work it out on its own
- About 20 seconds after the browser closes, the service exits on its own

---

## 2. Manager APK: features the original does not have

### 1. Random package name (hide the Manager)
KernelSU's answer to Magisk's "Hide the App". Because the package name is baked into the signed APK, the Manager cannot simply ship a second APK the way Magisk's stub does, so DikSU **rewrites the Manager on the device**:

- Rewrites `AndroidManifest.xml` (UTF-16LE, 5 places: the package, the dynamic receiver permission, the two authorities, and WebUI's taskAffinity)
- Rewrites `resources.arsc` (UTF-16LE, the package chunk's fixed-width name field)
- Rewrites `lib/<abi>/libksud.so` (UTF-8, ksud's own default package name, 2 places)
- Every replacement **keeps the byte length unchanged**, so no offset in either file moves
- Re-signs with the bundled `kernelsu.jks`. This is required: KernelSU crowns the Manager by the APK's signing certificate (`kernel/manager/apk_sign.c`, reached through `throne_tracker`)
- Supports `arm64-v8a` / `armeabi-v7a` / `x86` / `x86_64` / `riscv64`
- Custom app name and icon supported; the new app opens itself when done, and the plain Manager can be restored

### 2. Random package name for the launcher
Generates a fresh package name on every install for launcher APKs shaped like `com.xxxxx.yyyyy`. Five-letter segments keep the length identical to the original, so the offsets in `AndroidManifest.xml` and `resources.arsc` stay valid; if anything unexpected happens it falls back to installing the untouched APK.

### 3. One-tap Hide My Applist config
Runs the same HMA-OSS script straight from Settings (Standard / Scene), without opening the WebUI.

### 4. keyMint panel
A dedicated settings page (about 1000 lines) with the same content as the WebUI keyMint page: daemon status, property fix, app routing, keybox local / remote, log level, restart. If the `oh_my_keymint` module is not installed it shows the module id and an install hint.

---

## 3. Interface styles

- **Three interface styles**: `miuix (beautified)` / `miuix (stock)` / `Material 3`, switchable in Settings; stock and MD3 follow the official initial layout
- **Beautified UI**: image / video wallpaper backgrounds, glassmorphism navigation and panels, adjustable background dimming

---

## Credits

- Thanks to [Aster](https://github.com/LyraVoid/Aster) for the beautified UI design and implementation
- Thanks to the [KernelSU](https://github.com/tiann/KernelSU) project and community
- [Kernel-Assisted Superuser](https://git.zx2c4.com/kernel-assisted-superuser/about/): The KernelSU idea.
- [Magisk](https://github.com/topjohnwu/Magisk): The powerful root tool.
- [genuine](https://github.com/brevent/genuine/): APK v2 signature validation.
- [Diamorphine](https://github.com/m0nad/Diamorphine): Some rootkit skills.

## License

[GPL-3.0](/LICENSE), same as KernelSU.