# Kazama-Inc/libkrun：相对上游的差异

这是 `smol-machines/libkrun`（smolvm 维护的 libkrun 分支）的 fork。Kusa 桌面端的 Windows 运行时里的 `krun.dll` 从这里用源码编，不用 smolvm 仓库里提交的二进制。设计依据：`Kazama-Inc/kusa-desktop` 的 `docs/DESIGN.md` 第 10.5 节。

## 分支

| 分支 | 内容 |
|---|---|
| `main` | GitHub fork 时带过来的上游提交（`3d539d3a`），**原样不动、不作基线**：组织规则集管所有仓库的 `main`，禁止强推、只能走 PR 合入，没法钉到别处。libkrun 没有发布标签，基线是我们所用 smolvm 发布版本的子模块指针：smolvm v1.34.4 → `518949b4` |
| `kusa` | 基线 `518949b4` 加下表的补丁，**发布以它为准**（CI、smolvm fork 都认它）。smolvm fork 的 `kusa` 分支把 `libkrun` 子模块指到这里 |
| `fix/*`、`feat/*` | 每个补丁一个分支，从基线提交拉（不是从 `main`），能单独提给上游 |

升级：smolvm 换新发布标签时，取新标签的子模块指针作新基线，把下表的分支变基上去，再从新基线重做 `kusa`（`kusa` 不受规则集限制，可以重写）。

## 补丁

目标是只减不增：每条都提给上游，合进去一条删一条。

| 分支 | 内容 | 上游状态 |
|---|---|---|
| `fix/windows-virtiofs-escapes` | Windows virtiofs 服务端：① NTFS junction（及所有名字代理类重解析点）按符号链接呈现，指向共享根内的换算成相对路径；② 所有 `NtCreateFile` 带 `OBJ_DONT_REPARSE`，路径上遇到重解析点即 ELOOP；③ 拒绝含 `/`、`\`、`:` 的 guest 文件名（原来 `mv a '..\outside\x'` 能写到共享目录外）；④ rename / link / symlink 先核对目录路径上没有重解析点；⑤ guest 建的、指向共享根外的符号链接存成 WSL 式，主机程序不跟。另含让 `krun-devices` 的单测能在 Windows 上编译的一条提交 | 暂不提上游。① 是安全问题，先按 `smol-machines/smolvm` 的 `SECURITY.md` 私下报告（报告待发） |
| `fix/vsock-unix-prompt-close` | guest 关掉 unix 代理的连接时立即 `shutdown` 主机侧 socket，不等 reaper 的 5 秒宽限期。Windows 上宿主对自己的句柄 `shutdown` 叫不醒阻塞的 `recv`，取消一条命令原来要整 5 秒 | 暂不提上游 |
| `feat/windows-virtiofs-host-owner`（叠在 `fix/windows-virtiofs-escapes` 上） | `krun_add_virtiofs4` 新 flag `KRUN_VIRTIOFS_FLAG_HOST_OWNER`（`1 << 3`）：Windows 上没有记录属主的文件报 uid/gid 1000（`KRUN_VIRTIOFS_HOST_OWNER`）而不是 root，guest 才能把共享映射给非 root 工作负载。只给用户文件的共享用，镜像树不用；Linux、macOS 忽略 | 暂不提上游 |
| `feat/whp-lazy-guest-ram` | Windows（WHP）的 guest RAM 先保留不提交，guest 第一次碰到时按 2 MiB 提交，并用 `WHvAdviseGpaRange` 预填这一块（原来开机就按上限整块提交，再整块预填）；balloon 报告的空闲页直接 decommit，归还提交额度；主机内核态 I/O 之前先提交要用的 virtio 缓冲区（`descriptor_utils`、vsock 包）。默认开，`SMOLVM_WHP_LAZY_RAM=0` 恢复原行为 | 暂不提上游 |

## 构建（Windows，MSVC）

```bash
rustup target add x86_64-unknown-linux-musl          # 内嵌的 guest init 是 Linux 静态程序
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
cargo build --release -p libkrun --features blk,net   # -> target/release/krun.dll
cargo test  --release -p krun-devices --features blk,net --lib   # 含 escape_tests，不需要 WHP
```

上游自己的 Windows 构建是在 Linux 上用 mingw 交叉编（`x86_64-pc-windows-gnu`）。我们定用 MSVC 原生编（11-14 用户确认）。原以为 MSVC 版偶发「流式命令 1 秒内没起来」，查明是 Go SDK e2e 的测试问题（心跳只精确到秒），与编法无关。

`escape_tests` 的夹具建在 `%TEMP%` 下；GitHub runner 的 `%TEMP%` 是 8.3 短名（`RUNNER~1`），本地复现要把 `TMP`/`TEMP` 指到一个短名路径。
