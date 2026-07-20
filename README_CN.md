# KMiri

KMiri 是一个基于 Miri 的 Rust 内核执行与检查环境。它让内核代码可以运行在 Miri 的 MIR 解释器中，同时补充普通 Miri 不具备的内核执行模型。

本仓库主要包含两部分：

- **kmiri**：Miri fork，以及面向内核的执行和检查支持。
- **MIR debugger**：用于在 MIR/解释器层面检查 kmiri/Miri 运行状态的交互式调试器。

## kmiri

kmiri 的重点是让内核内存管理和调度相关代码可以在 Miri 中执行，而不是把它们简化成普通用户态 Rust 程序。核心功能如下。

### 伪物理内存

kmiri 会保留一段伪物理内存区域，并以页为粒度进行管理。由 Rust 运行时管理的分配，例如栈和静态变量，会放在 kernel-code 区域；由 OS 自己管理的内存，则可以表示为由具体物理页作为后端的 Miri allocation。

这样一来，即使内核代码在指定物理地址上创建对象，也仍然可以使用 Miri 的 allocation、provenance、初始化、边界和生命周期检查。

### 页状态跟踪

kmiri 会跟踪物理页状态，并根据这些状态验证内存访问是否合法。内核在分配、释放页面，或者把页面转换为某种用途时，需要通知 kmiri。

这一机制用于发现普通 Miri 看不到的内核级内存错误，包括：

- 访问 free 或 reserved 状态的物理页。
- 通过普通 typed memory 操作访问 untyped page。
- 执行非法的 untyped memory 读、写或拷贝。
- 以违反页状态模型的方式复用或释放页面。

### 页表模型

kmiri 会模拟分页架构，并维护当前活跃页表。内核代码可以设置或查询根页表；kmiri 在解释内存访问时，会遍历当前活跃页表。

这使得页表错误可以在解释执行过程中暴露出来，例如非法映射、过期映射、错误的页表更新，以及同一个物理页通过多个映射暴露后破坏内核内存安全假设的情况。

### 地址感知的 Allocation

kmiri 会把 Miri allocation 绑定到伪物理内存中的地址。Rust 运行时创建的 allocation 会在需要时迁移到伪物理内存后端；由内核创建的 typed memory 可以直接在某个页面上分配。

因此，即使内核通过显式地址操作内存、在地址范围之间拷贝字节，或者在 OS 管理的页面上构造 typed object，Miri 原有的 UB 检查仍然有效。

### 内核线程模型

kmiri 为内核管理的线程提供 shim。内核创建线程、切换线程，或者使用线程入口和栈初始化应用处理器时，可以通知 kmiri。

每个模拟线程都有自己的栈区域，因此栈分配会在内核调度器预期的地址空间中被解释执行。

### SMP 和 CPU-Local 数据

kmiri 支持多 CPU 和 CPU-local storage。CPU-local 变量会被放入一段保留的伪物理内存区域，模拟 CPU 可以绑定到内核线程。

kmiri 可以在多个模拟 CPU 之间随机切换，用于触发容易出现 race 的路径，同时保留每个 CPU 上由内核显式调度器控制的执行行为。

### 内核 Shim API

kmiri 暴露了一组 shim API，内核代码通过它们把底层操作同步给解释器模型：

- 页管理：分配、释放物理页，以及设置物理页类型。
- Untyped memory 操作：通过显式 helper 读、写和拷贝 untyped memory。
- 页表管理：设置和获取当前活跃根页表。
- 线程和 CPU 管理：创建线程、切换线程，以及初始化模拟 CPU。

这些 API 刻意保持较小规模：它们只向 kmiri 提供维护内存和执行模型所需的事件，而不是模拟一台完整机器。

### kmiri 检查什么

kmiri 保留 Miri 原有的 Rust UB 检查，并把检查范围扩展到内核执行场景：

- Use-after-free，以及悬垂指针或悬垂引用的使用。
- 越界和未对齐内存访问。
- 未初始化读取。
- 非法 pointer provenance。
- 违反 Miri borrow model 的 aliasing。
- 在配置的 checker 能够观测到时检查 data race。
- 会以不安全方式暴露 typed memory 的页表和映射错误。
- 对 untyped、free 或 reserved 物理页的非法访问。
- OSTD/Asterinas 内存抽象中破坏 ownership、初始化和清理不变量的问题。

kmiri 是执行时检查工具。只有实际被解释执行到的路径，才可能被报告出问题。

## 运行

安装本地 Miri 工具，并通过 OSDK 集成运行目标内核测试：

```bash
cd kmiri
./miri install --debug
cd ../tests/init
OSDK_LOCAL_DEV=1 cargo osdk miri test
```

具体命令可能会随着所选 Asterinas、OSDK、Rust nightly 和 kmiri 分支变化。请使用当前工作区固定的版本组合。

## MIR Debugger

![](https://github.com/user-attachments/assets/825d7ee5-f11a-46fd-9f60-469d35a23b84)

![](https://github.com/user-attachments/assets/db27e754-0a9c-4fb4-8e48-98260783d8ad)

MIR debugger 是 kmiri 内置的 TUI 调试器。通过给 Miri 传入 `--debugger` 启用，用于检查解释器快照、控制 MIR 执行，并运行到指定目标，例如函数 instance、源码行或 MIR terminator。

例如：

```bash
./miri run tests/pass/debugger_test.rs --debugger
```

通过 OSDK 运行时，把同一个 flag 放进 `MIRIFLAGS`：

```bash
OSDK_LOCAL_DEV=1 MIRIFLAGS="--debugger" cargo osdk miri test
```

对 `tests/init` 采用 TUI 调试的完整命令：

```bash
./tool/install_miri_osdk.sh

cd tests/init
OSDK_LOCAL_DEV=1 MIRIFLAGS="--remap-path-prefix=$(rustc --print=sysroot)/lib/rustlib/src/rust/library/= --remap-path-prefix=$(realpath ../../asterinas)/=" cargo osdk miri-debugger
```

### Panes

TUI 由多个 pane 组成。当前 focus 的 pane 会高亮边框，`Tab` / `Shift+Tab` 用于在主 pane 之间切换 focus。

| 面板 | 展示内容 |
| --- | --- |
| MIR | 当前 basic-block CFG 概览，以及当前位置的 MIR statements/terminator。当前 MIR 行会高亮，terminator 会使用单独样式。 |
| Stack | 当前线程的调用栈 frame、函数名、源码位置、选中的 frame、当前 thread ID，以及可用时的 stack pointer 信息。 |
| Source | 当前 MIR 位置对应的源码片段，高亮当前 source range，并在标题中展示文件和行号范围。 |
| Locals | 选中 stack frame 的 locals。列包括 local ID、源码名、类型和值；值会根据 dead、uninitialized、pointer、initialized 等状态着色。 |
| Allocations | Miri allocations。列包括 `AllocID`、base physical address、是否 deallocated、memory kind、大小、对齐、是否 exposed provenance、global 名称和相关 locals。 |
| Output | debugger 启用期间，被解释程序写出的 stdout/stderr。 |
| Instances | 可搜索的 function/MIR instances，以及对应源码文件和行号范围。该 pane 也用于 run-to-instance 和 run-to-source-line。 |
| Status bar | 当前 run mode、step count、thread ID、focused pane、history 使用量、record 模式、instance search 状态、run target 和上下文相关快捷键提示。 |
| Borrow Stacks | 按 `s` 打开的 modal pane。展示 allocations 的 Stacked Borrows 或 Tree Borrows 状态；选中 borrow item 时，如果有对应 tag 的 source span，会显示相关源码。 |

### 快捷键

| 快捷键 | 含义 |
| --- | --- |
| `q` | 退出 debugger。 |
| `n` | Step over 一个 debugger snapshot。可以加数字前缀，例如 `10n`。 |
| `Space` | Step 到当前 frame 的下一个 MIR terminator。可以加数字前缀重复多个 terminator stop。 |
| `b` | 在已记录的 debugger history 中向后查看。可以加数字前缀回退多个 snapshot。 |
| `c` | Continue，继续执行并跳过中间 debugger snapshots。 |
| `e` | Run to end，运行到程序结束。 |
| `t` | Run to MIR terminator。可以加数字前缀跳过多个 terminator hit。 |
| `/` | Focus 到 Instances，并进入 instance search。 |
| `?` | Focus 到 Instances，清空当前 query，并进入 search。 |
| Instances 中按 `Enter` | 运行到选中的 instance。如果 query 包含 `.rs`，则把它当作 `path/to/file.rs` 或 `path/to/file.rs:line` 这样的源码目标。 |
| Instances 中按 `.` / `,` | 跳到下一个 / 上一个 search match。 |
| Instances 中按 `Esc` | 清空当前 instance search query。 |
| `[` / `]` | 横向滚动 status bar 中的命令/帮助文本；编辑 instance search 时表示上一条 / 下一条 search history。 |
| `S` | 切换是否记录所有中间 debugger states。 |
| `F` | 切换 freeze mode。启用时，MIR 和 Source panes 会自动居中到高亮位置。 |
| `D` | 切换是否显示 dead allocations，以及 allocation/borrow 视图中的 dead 数据。 |
| `s` | 打开或关闭 Borrow Stacks modal。 |
| `Tab` / `Shift+Tab` | 切换到下一个 / 上一个主 pane。 |
| `Up` / `Down` | 在 focused pane 中导航或滚动。Stack 和 Instances 中会改变选中项。 |
| `PageUp` / `PageDown` | 对 focused pane 翻页滚动，或在 Instances 中翻页选择。 |
| `Left` / `Right` | 对 focused pane 横向滚动。 |
| 鼠标滚轮 / 点击 | focus 到鼠标所在 pane 并滚动；在 Borrow Stacks 内点击会选中对应行。 |

数字前缀会在可消费计数的执行/导航快捷键前累积，主要包括 `n`、`Space`、`b` 和 `t`。

### 执行方式

- **Step over**：按 `n` 发送 `StepOver`，按 debugger snapshot 前进。处于 history replay 时，`n` 会先在已记录 snapshot 中向前移动，再请求解释器继续执行。
- **Step frame terminator**：按 `Space` 运行到当前 frame 的 MIR terminator。如果执行进入 callee，会等控制流回到锚定 frame 后再停止。
- **Run to terminator**：按 `t` 运行到 MIR terminator；可以用数字前缀跳过多个 terminator hit。
- **Run to instance**：在 Instances 中搜索或选择条目后按 `Enter`；当 active frame 的函数 instance 匹配目标时停止。
- **Run to source line**：在 Instances search 中输入 `path/to/file.rs` 或 `path/to/file.rs:line` 这样的源码目标后按 `Enter`；当当前 frame 匹配该文件，并且在提供行号时也匹配该行号，就会停止。
- **Continue**：按 `c` 切换为连续执行，不展示中间 debugger snapshots。
- **Run to end**：按 `e` 继续执行到程序结束。
- **Reverse view**：按 `b` 在已记录 debugger snapshots 中向后查看。这是 TUI history navigation，不是解释器级反向执行。

### 断点和停止方式

当前 TUI debugger 使用 run-to target，而不是持久化 breakpoint table。支持的停止目标包括：

- **Function/MIR instance**：使用 Instances search（`/` 或 `?`），选中 instance 后按 `Enter`。
- **源码行**：在 Instances search 中输入 `path/to/file.rs` 或 `path/to/file.rs:line` 这样的源码目标，然后按 `Enter`。
- **MIR terminator**：按 `t` 停在 terminator，或按 `Space` 停在当前 frame 的 terminator。
- **程序结束**：按 `e`。
- **解释器失败**：UB 报告、panic、不支持的操作和异常终止由 kmiri/Miri 的正常诊断路径报告。TUI 不提供独立的持久化失败状态保存机制。

## 当前状态

kmiri 仍处于实验阶段，并且与 Rust nightly、Miri 内部实现、Asterinas 和 OSDK 紧密相关。遇到失败时，需要判断它属于哪一类：

- 被检查内核代码中的真实 bug。
- kmiri 模型缺失或不准确。
- Miri 当前的限制。

kmiri 适合作为内核 unsafe 抽象的开发期检查工具。它不是完整的机器模拟器，不执行汇编，不像 QEMU 那样模拟设备，也不会证明没有被执行到的路径。
