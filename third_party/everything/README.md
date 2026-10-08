# Everything SDK

技术验证 `spikes/fileidx` 动态加载的 x64 DLL。固定版本，不在运行时下载。

两个压缩包都在 2026-10-08 从 voidtools 官网下载。许可原文在各目录的 `LICENSE.txt`。条款是同一份无担保的许可：可以免费使用、复制、修改、合并、发布、分发、再许可和销售，须保留版权声明和许可声明。

| 文件 | 对应客户端 | 下载 | SHA-256 |
| --- | --- | --- | --- |
| `sdk/Everything64.dll` | Everything 1.4，SDK 版本 2 | <https://www.voidtools.com/Everything-SDK.zip> | `81b5be18126acd2c2b913f8f4a821e476b18393cdd3debd03387c50afd8db88f` |
| `sdk3/Everything3_x64.dll` | Everything 1.5，SDK 3.0.0.9 | <https://www.voidtools.com/Everything-SDK-3.0.0.9.zip> | `be25b01c73bbf359b50ddf30255133225f93b4bc40a8d208173319373bcdaa5c` |

压缩包本身的 SHA-256：

| 压缩包 | SHA-256 | 大小 |
| --- | --- | --- |
| Everything-SDK.zip | `f5716d9513cce6b462b5170a0a2e7e081e191d9bcac6774f5decc061497df443` | 238231 字节 |
| Everything-SDK-3.0.0.9.zip | `124685d35a5f49f3c1e9898853e166215748c893782c6a251f5dde58dacad4fa` | 515026 字节 |

仓库只放 x64 DLL 和许可说明，不放压缩包、导入库和示例工程。

## 版本

- `Everything64.dll` 没有 VERSIONINFO。`include/Everything.h` 里 `EVERYTHING_SDK_VERSION` 是 2。voidtools 说明版本 2 对应 Everything 1.4。头文件版权年份是 2016，编进 DLL 的 `src/Everything.c` 版权年份是 2022。
- `Everything3_x64.dll` 的 FileVersion 和 ProductVersion 都是 3.0.0.9，FileDescription 是 `Everything SDK`，LegalCopyright 是 `Copyright (C) 2025 voidtools`。`src/version.h` 是 `VERSION_MAJOR 3`、`VERSION_MINOR 0`、`VERSION_REVISION 0`、`VERSION_BUILD 9`。

## 调用

`LoadLibraryW` 加载上面的 DLL。探测顺序是 SDK3（1.5）再 SDK（1.4）。SDK3 先连未命名实例，管道不存在再连 voidtools 文档里的 `1.5a`（1.5 alpha 的实例名）。1.4 这份 DLL 只找窗口类 `EVERYTHING_TASKBAR_NOTIFICATION`，没有实例名参数。

查询最多取 50 条。状态区分见 `spikes/fileidx`。
