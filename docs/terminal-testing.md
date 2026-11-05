# 终端验收

## 自动检查

先运行格式、Clippy 和 Rust 测试，再构建待验收版本：

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release --locked
python3 scripts/check_stream_pty.py --binary target/release/tmd
python3 scripts/test_stream_pty.py --binary target/release/tmd
python3 -B scripts/test_reader_pty.py --binary target/release/tmd
python3 -B -m unittest discover -s scripts -p 'test_release.py'
```

PTY 检查只使用 Python 标准库和 `/bin/bash`，适用于 macOS/Linux；不安装依赖，不读取用户 shell 配置，不保存 shell 历史。可用 `--binary <路径>` 指定构建产物。

它发送带中文和 emoji 的分段 UTF-8 代码块，检查：

- stdout 是终端时产生多个同步输出帧。
- 从 80 列缩到 60 列后，输出确实使用新的宽度。
- 正常结束返回 0；Ctrl-C 结束返回 130，shell 能继续执行命令。
- 光标、设置过的 Unicode 版本和终端属性恢复。

`test_stream_pty.py` 另覆盖 stdin 保持打开时的 INT、TERM、HUP、QUIT，分别检查纯文本和 Claude 流式退出码及恢复序列。

输出每个场景的 JSON 检查结果；失败返回非零。探测用 `TERM_PROGRAM=Kaku` 覆盖协议分支，不表示运行在真实 Kaku 中。

CI 在 Linux/macOS 上构建 debug 二进制并执行同一探测，包含纯文本和 Claude JSONL 各自的正常结束、缩放和中断。缩放和中断以已经观察到的输出为触发条件，避免依赖机器启动速度。探针使用足够超出活动区的 24 行代码，不把数千次短 sleep 的总耗时当成产品性能要求；长文档仍由 Rust 流式测试和下方真实终端检查覆盖。shell 关闭行编辑，避免 C locale 将 UTF-8 命令当成 Meta 快捷键。生产器通过脚本入口启动，避免内联代码超过 macOS 的终端输入行长度限制。首行显示后，父进程先缩放再通过管道放行剩余输出，避免渲染器启动较慢时全部样本提前输出。回归测试覆盖 C locale、父进程晚运行、渲染器延迟启动，以及 `--debug` 在共享终端输出诊断时的活动区清理。

## 真实终端检查

PTY 通过不能证明无闪烁、字体宽度和终端兼容性。在 Kaku 主测，并在 Ghostty、kitty 补测：

```sh
python3 scripts/slowcat.py fixtures/long.md | target/release/tmd
target/release/tmd --width 80 fixtures/cjk-table.md
```

检查长输出不闪、中文/emoji 表格对齐、代码块逐行出现、活动区不超过终端高度，以及正常结束后光标可用。流式期间缩放窗口，确认只重排活动区；再运行一次并按 Ctrl-C，确认上游结束、光标和输入正常。

记录实际使用的终端、执行版本和结果；未运行的终端明确标为待验收。自动检查、视觉验收和作者接受分别报告。

## 静态大标题

以下命令使用非交互 stdin 测试一次性输出；交互阅读器另按下节验收。

在 Kaku 直接运行，先保持至少 40 列、10 行：

```sh
target/release/tmd --large-headings fixtures/large-headings.md < /dev/null
target/release/tmd fixtures/large-headings.md < /dev/null
target/release/tmd --large-headings --width 20 fixtures/large-headings.md < /dev/null
```

逐项记录结果，截图和验收记录保存于仓库外：

1. 对比开启和关闭：顶层 ATX、Setext H1 宽高各两倍，H2 至 H6、引用/列表中的 H1、正文、代码和表格维持常规字号；H1 分隔线全宽且普通高度。
2. 检查中文、组合字符、emoji、带样式和链接的标题，20 列换行不截断字符、不重叠；文档超出窗口高度并滚入历史时上下半行仍配对。
3. 输出结束后输入 `printf 'normal prompt test\n'`，确认提示符和后续文字大小正常；缩放窗口后再次检查后续输出。静态历史不承诺重新排版。
4. 选中并复制放大标题到纯文本编辑区，记录是否出现两份文字。标准上下半行写有相同内容，复制重复是已披露限制，不能声称已无损复制。
5. 将样本重复成较长临时文件，使用 `< /dev/null` 执行大标题输出并在输出期间按 Ctrl-C；检查退出码为 130，后续提示符普通大小，没有孤立的半个标题。不要向仓库写入临时长文件。
6. `--ansi`、输出重定向、管道输入及 tmux/screen 中均应为普通标题；不能手动伪造 Kaku 环境变量来声称完成真实显示验收。

`scripts/test_stream_pty.py` 包含静态正常结束、收到实际大标题输出后 Ctrl-C、过窄/过矮窗口及能力回退检查；Rust 测试在每个输出字节边界注入一次写入错误或取消，检查恢复序列。真实 VINTR 回归向控制终端写入 `0x03`，检查临时 `NOFLSH`、关闭回显、保留 canonical/ISIG，以及退出后恢复原 termios；另测 HUP、QUIT、TERM 的恢复与退出码。这些只证明输出协议与生命周期，真实 Kaku 显示和剪贴板结果须单独记录。

## 主屏滚动阅读

自动检查：`python3 -B scripts/test_reader_pty.py --binary target/debug/tmd`，与已有流式/大标题回归同时运行。

Kaku 手动检查：

1. 从窗口顶部、中间、底部运行 `target/debug/tmd fixtures/acceptance.md`，确认顶部有调用信息，下面是文章开头，没有切备用屏。
2. 用滚轮连续上下阅读，Home/End 到首尾；不需要按空格才能加载后文。按 Shift 拖选并复制普通文字。短文档也保持打开；放大窗口不会退出。按 `?` 查看帮助，在帮助中滚动后按 q/Esc 回到原阅读位置；粘贴含 q 的文本不退出。
3. 用 `--large-headings fixtures/large-headings.md` 检查大标题跨视口边界不会留下半行，滚动及缩放后正文不变大。
4. 缩放至窄窗口及仅一行高，再恢复；检查段落定位与退出仍可用。q/Esc 后提示符正常，Ctrl-C 返回 130；输入后续命令确认回显正常。向上查看原会话历史仍在。
5. 按 `/` 使用系统拼音输入中文查询，确认候选窗跟随输入位置，Enter 后显示匹配计数并定位命中行；n/N 循环跳转。查询中输入 q 不退出；Esc 取消，缩放后取消仍回原位置。检查跨软换行词、无结果、帮助与搜索状态的 Esc，以及搜索中 Ctrl-C 恢复。
6. 按 `c` 选择含 Tab、中文和长行的代码块，明确执行复制后粘贴到纯文本文件，与代码内容比较；检查 CRLF 转 LF、嵌套代码、末尾换行、空块不改剪贴板。测试前确认个人剪贴板内容已妥善保存；自动测试只用假后端。取消、滚动、搜索、退出不应改剪贴板。代码选择中缩放和 Ctrl-C 后检查终端恢复。
7. 按 `o` 查看 H1–H6 和 Setext 标题目录；引用、列表和代码中的示例标题不应出现。分别跳转重复标题，检查到达各自章节；选择中缩放后 Enter 跳转仍正确，双高 H1 成对显示。q/Esc 返回原阅读和搜索状态；目录中粘贴含 Enter/q 的内容不触发操作，Ctrl-C 后终端输入恢复。
8. 配置文件、CLI 覆盖、错误配置分别检查。`--ansi`、文件重定向、纯文本和 Claude JSONL 管道维持原行为。
9. 原生增强用仓库外临时 Kaku 配置先验收，不能覆盖个人配置：探测成功后全文输出并定位开头；未装适配器或历史容量不足时在全文输出前回退。复制和滚动由终端处理，结束后没有多余输入落入 shell。

记录自动检查、Kaku 视觉验收及作者接受三个独立结论。连接失败或未安装适配器时不得将模拟握手记为原生增强通过。
