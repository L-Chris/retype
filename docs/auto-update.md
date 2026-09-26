# 自动更新机制

参考实现：`F:/projects/ebook/torto-app/lib/app/update/app_update_service.dart`
（查 GitHub `releases/latest` → 比对版本 → 给出 release 页）。
retype 沿用同一套契约，但做了三处针对输入法的改动。

---

## 1. 与参考实现的对照

| 参考实现（torto-app） | retype | 为什么 |
|---|---|---|
| Dart，跑在 app 进程里 | Rust `core/updater` + 独立 `retype-updater.exe` | TIP DLL 被注入每个宿主进程，绝不能在里面发 HTTP（见下） |
| `http.Client` 可注入 | `HttpFetcher` trait 可注入 | 同样为了可离线单测 |
| 按点切分整数比版本 | 完整 semver（含预发布优先级） | `0.2.0-rc.1` 在参考实现里会和 `0.2.0` 比出错误结论 |
| 404 → 无 release，不报错 | 同 | 新项目在打第一个 tag 之前，检查更新应该安静返回 |
| 15s 超时 | 同（写在 `HttpFetcher` 契约里） | 检查更新卡在设置界面上等于界面死掉 |
| 只给 release 页，不下载 | 给 release 页 **且**支持下载 + sha256 校验 | 桌面端值得多做一步；但校验是硬门槛 |

---

## 2. 为什么更新逻辑不能放在 TIP DLL 里

TIP 是 in-proc COM DLL，由系统注入到**每一个**需要文本输入的进程。在里面做更新检查意味着：

- Chrome、Word、记事本各自替我们打一遍 API 请求，各持一条 TLS 连接；
- 请求发生在宿主的进程空间和线程上，任何阻塞都直接违反 **P1**（输入主线程不得阻塞）；
- 宿主的代理/证书/防火墙策略各不相同，失败原因无法归因；
- 我们崩溃就是宿主崩溃。

所以：**更新是一个独立的 exe**，由设置界面（Flutter）或计划任务调起。
`core/updater` 是纯逻辑 crate，不含任何网络实现，Android 端可以原样复用。

```text
apps/settings (Flutter)  ──调用──►  retype-updater.exe  ──HTTPS──►  GitHub API
      ▲                                     │
      └────────── --json 输出 ───────────────┘
                                            │
                              core/updater（纯逻辑 + HttpFetcher trait）
```

---

## 3. 流程与信任边界

```text
check ──► GET /repos/{owner}/{repo}/releases/latest
            │
            ├─ 404            → Ok（还没有 release，不是错误）
            ├─ 其他非 2xx      → Err（调用方按「这次查不到」处理，绝不影响打字）
            └─ 200            → 解析 tag / html_url / assets
                                  │
                        通道过滤（stable 跳过 prerelease）
                                  │
                        版本比较 latest > current ?
                                  │否 → 静默结束（不打扰用户）
                                  │是
                                  ▼
download ──► 先取 <产物>.sha256 ──► 再取产物 ──► verify_sha256
                                                    │失败 → 丢弃，退出码 4，磁盘上不留文件
                                                    │通过
                                                    ▼
                                        落盘到 --out 目录
                                                    │
                                                    ▼
                          静默运行 setup.exe /VERYSILENT /NORESTART
                          （Restart Manager 处理 DLL 被占用，见 §4）
```

三条硬性规则：

1. **校验先于落盘**。顺序是「先拿 sha256，再拿产物，校验通过才写文件」，
   磁盘上永远不会出现一个未经验证、却看起来像是对的产物。
2. **缺校验文件 = 不可安装**。`UpdateStatus::is_installable()` 要求产物和
   `.sha256` **成对存在**；只发安装包不发校验文件时，更新器只会把用户引导到 release 页。
3. **只接受 https**。`parse_release_json` 会拒绝非 https 的 `html_url`，
   否则更新通道可以被降级成明文。

---

## 4. 安装与「替换正在使用的 DLL」

TIP DLL 正被所有宿主进程加载着，直接覆盖会失败（文件占用）或造成半更新状态
（有的进程用新版、有的用旧版，用户词库格式一旦变化就会互相写坏）。

**这件事交给 Inno Setup 安装器，不要自己发明机制。**

> 早期设计里我打算做「版本化目录 + 注册表指向新版本」（`%LOCALAPPDATA%\retype\versions\0.2.0\`
> 加一个注册表值原子切换）。**这个方案已作废**：Windows 的 Restart Manager 就是专门
> 解决「文件被 N 个进程占用」的标准机制，Inno Setup 已经接好了它，
> 自己再造一套只会更脆弱，还要自己处理回滚、卸载条目、占用检测、重启后替换。

安装器负责的事（`platforms/windows/installer/retype.iss`）：

| 事项 | 做法 |
|---|---|
| DLL 被占用 | `CloseApplications=yes` → Restart Manager 列出占用进程让用户选择关闭；关不掉的自动登记为「重启后替换」，结束页提示重启 |
| 不擅自重启用户的应用 | `RestartApplications=no`（升级时突然重开一堆 Chrome 标签页不可接受） |
| HKLM 注册 | `[Registry]` 段写 CTF 的 `InprocServer32` + `LanguageProfile`，卸载时由 `uninsdeletekey` 反向清理 |
| 词库落位 | 装到 `{app}\retype-dict.tsv`（DLL 同目录）。TIP 的工作目录是宿主进程的，所以 `session.rs` 用 `GetModuleHandleExW(FROM_ADDRESS)` 定位自己的目录来找词库 |
| 跨版本升级 | `AppId` 固定不变，Inno 据此识别为同一产品并原地升级 |
| 用户数据 | 卸载时询问是否删除 `%LOCALAPPDATA%\retype`（默认否 —— 用户积累的选词习惯比一次干净卸载更值钱） |
| 装完看不到输入法 | 结束页明确告知必须注销重登（TSF 的语言配置档被登录会话缓存），避免被当成安装失败 |

因此**自动更新的最后一步就是静默跑安装器**：

```powershell
retype-updater.exe download --out "$env:TEMP\retype-update"
# 校验已在 download 内部完成（不通过就退出码 4，且不会落盘）
& "$env:TEMP\retype-update\retype-<新版本>-windows-x64-setup.exe" `
    /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP-
```

`/NORESTART` 是刻意的：由调用方（设置界面）决定何时提示用户重启，
而不是让安装器在用户打字打到一半时重启机器。

### 仍未解决的部分（M5）

- **代码签名**：目前没有证书，未签名的 setup.exe 会触发 SmartScreen 警告。
  这是发布消费级软件前必须解决的，否则用户看到「Windows 已保护你的电脑」基本就放弃了。
- **静默升级期间的输入**：升级会替换掉正在使用的 TIP DLL。即使 Restart Manager
  处理了文件占用，用户当前正在组字的内容仍可能丢失。M5 需要在升级前主动
  commit/cancel 掉所有活跃组字会话，并考虑「延迟到用户空闲时再升级」。
- **升级触发时机**：设置界面打开时检查 + 每日计划任务，注意 GitHub 匿名限额
  60 次/小时/IP（`GITHUB_TOKEN` 可提到 5000）。

---

## 5. 用法

```powershell
# 检查更新（退出码 0=已最新，10=有更新，2=网络错误）
retype-updater.exe check

# 给设置界面用的 JSON 输出
retype-updater.exe check --json

# 下载并校验到指定目录
retype-updater.exe download --out "$env:TEMP\retype-update"

# 指定仓库 / 版本 / 平台 / 通道
retype-updater.exe check --repo acme/retype --current 0.1.0 --platform windows-x64 --channel beta
```

仓库地址的解析优先级：

```text
--repo 参数  >  环境变量 RETYPE_GITHUB_REPO  >  编译期烘入值
```

CI 在构建时把 `github.repository` 通过 `RETYPE_GITHUB_REPO` 环境变量注入，
`core/updater` 用 `option_env!` 烘进二进制 —— 发布出去的 exe 天然知道自己的仓库，
而开发构建（没这个环境变量）会明确报「仓库未配置」，不会悄悄打到一个占位地址。

GitHub API 匿名限额是 60 次/小时/IP。如果更新检查放在计划任务里高频跑，
需要配 `GITHUB_TOKEN`（`--token` 或同名环境变量）。

---

## 6. 测试策略

`core/updater` 的 38 项测试全部离线跑，用 `MockHttp` 注入故障：

| 场景 | 断言 |
|---|---|
| 有新版 / 已最新 / 本地更新 | `update_available` 三态正确，不提示降级 |
| 404 | `Ok` 且 `latest == None`（新仓库不算错误） |
| 403 限流 / 500 | `Err(Http{status})`，不静默成「没有更新」 |
| 传输失败（断网） | `Err(Transport)` |
| 坏 JSON / GitHub 错误体 | `Err(Malformed)`，且保留 GitHub 的原始 message |
| 非 https 的 release URL | 拒绝 |
| 预发布版本 | stable 通道跳过，beta 通道可见 |
| 缺产物 / 缺 sha256 | `is_installable() == false` |
| sha256 不匹配 | `ChecksumMismatch`，带上两个值 |
| sha256 格式非法 | `Malformed` |
| 仓库地址非法（8 种畸形输入） | `RepoNotConfigured`，且**不发请求** |
| 版本号非法 | `InvalidVersion`，且**不发请求** |
| NIST sha256 标准向量 | 确认没把哈希算成别的什么 |
| semver 预发布优先级 | `rc.10 > rc.2`、`rc < rc.1`、数字段 < 字母段 |

「不发请求」这两条很重要：版本号或仓库地址配错时就应该立刻失败，
而不是先打一次网络请求再报错 —— 前者能在启动瞬间定位问题，后者会被误诊成网络故障。
