# 安装程序附带组件

## `bonjour.msi` —— Apple Bonjour 3.0.0.10 (x64)

iPhone 通过 mDNS 在局域网里发现接收端，在 Windows 上这件事由 Apple 的 Bonjour
承担：协议库 `LoadLibrary("dnssd.dll")` 并与 `mDNSResponder.exe` 服务通信。缺了它，
接收器能正常启动，但手机永远搜不到本机。

`bonjour-hook.nsh` 在安装的 `NSIS_HOOK_POSTINSTALL` 阶段检查 64 位
`System32\dnssd.dll`；只有在缺失时才**询问用户**并调用 `msiexec /passive` 安装。
它**不会**在卸载 Mirrwin 时移除 Bonjour —— 那是系统共享组件，iTunes、打印机驱动
以及其他 AirPlay 接收端都可能正在使用。

即使用户拒绝安装，Mirrwin 本身也会正常装完；随后启动接收器时程序会给出明确提示
（见 `docs/ffi-contract.md` 的 `-3` 返回码）。

### 这个文件的来源

| 项 | 值 |
| --- | --- |
| ProductName | Bonjour |
| ProductVersion | 3.0.0.10 |
| Manufacturer | Apple Inc. |
| ProductCode | `{6E3610B2-430D-4EB0-81E3-2B57E8B9DE8D}` |
| 架构 | x64 |
| SHA-256 | `53be81cc6e2dc95a1041e8f3d8f500fad4259ab20a1aac151b5fc7a64d354a93` |

取自安装 Bonjour 后 Apple 自己留下的安装缓存
（`%ProgramData%\Apple\Installer Cache\Bonjour 3.0.0.10\bonjour.msi`），是自包含的
MSI（无外部 cab）。

### ⚠️ 分发许可

**Bonjour 是 Apple 的软件，本项目只是随包附带，并不拥有其版权。** 对外分发前请自行
确认符合 Apple 的再分发条款（历史上 Apple 通过 *Bonjour SDK for Windows* 的许可协议
允许随应用分发 Bonjour 安装程序，但条款以 Apple 当前发布的为准）。

如果不希望承担这一分发责任，去掉 `tauri.conf.json` 里的
`bundle.windows.nsis.installerHooks` 即可：安装程序会退回到"不附带 Bonjour"，
程序在运行时仍会明确提示用户自行安装。
