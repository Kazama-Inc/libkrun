# Kazama-Inc/libkrun：相对上游的差异

这是 `smol-machines/libkrun`（smolvm 维护的 libkrun 分支）的 fork。Kusa 桌面端的 Windows 运行时里的 `krun.dll` 从这里用源码编，不用 smolvm 仓库里提交的二进制。设计依据：`Kazama-Inc/kusa-desktop` 的 `docs/DESIGN.md` 第 10.5 节。

## 分支

| 分支 | 内容 |
|---|---|
| `main` | 跟上游。libkrun 没有发布标签，`main` 钉在我们所用的 smolvm 发布版本的子模块指针上：smolvm v1.34.4 → `518949b4` |
| `kusa` | `main` 加下表的补丁。smolvm fork 的 `kusa` 分支把 `libkrun` 子模块指到这里 |
| `fix/*`、`feat/*` | 每个补丁一个分支，从 `main` 拉，能单独提给上游 |

升级：smolvm 合新发布标签时，把 `main` 快进到新标签的子模块指针，再把下表的分支变基上去、重做 `kusa`。

## 补丁

目标是只减不增：每条都提给上游，合进去一条删一条。

| 分支 | 内容 | 上游状态 |
|---|---|---|
| `fix/windows-virtiofs-escapes` | Windows virtiofs 服务端：① NTFS junction（及所有名字代理类重解析点）按符号链接呈现，指向共享根内的换算成相对路径；② 所有 `NtCreateFile` 带 `OBJ_DONT_REPARSE`，路径上遇到重解析点即 ELOOP；③ 拒绝含 `/`、`\`、`:` 的 guest 文件名（原来 `mv a '..\outside\x'` 能写到共享目录外）；④ rename / link / symlink 先核对目录路径上没有重解析点；⑤ guest 建的、指向共享根外的符号链接存成 WSL 式，主机程序不跟。另含让 `krun-devices` 的单测能在 Windows 上编译的一条提交 | 草稿，未提 |

## 构建（Windows，MSVC）

```bash
rustup target add x86_64-unknown-linux-musl          # 内嵌的 guest init 是 Linux 静态程序
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
cargo build --release -p libkrun --features blk,net   # -> target/release/krun.dll
cargo test  --release -p krun-devices --features blk,net --lib   # 含 escape_tests，不需要 WHP
```

上游自己的 Windows 构建是在 Linux 上用 mingw 交叉编（`x86_64-pc-windows-gnu`），我们用 MSVC 原生编；两者在 Windows e2e 上表现一致。
