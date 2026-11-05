# tmd

终端里的 Markdown 渲染器，命令 `tmd`。只读，主打流式不闪、中英文对齐、主题。

面向日常试用的早期版本，优先在 Kaku 中检查阅读和流式输出效果。

## 源码安装

需要 Git 和 Rust 稳定版工具链。将构建产物安装到 `~/.local/bin`；已有同名 `tmd` 会被替换。

```sh
git clone git@github.com:HiAriesZhou/tmd.git
cd tmd
cargo build --release --locked
mkdir -p "$HOME/.local/bin"
install -m 755 target/release/tmd "$HOME/.local/bin/tmd"
```

若该目录尚未在 PATH 中，先执行以下命令；长期使用时可将它加入 shell 配置。

```sh
export PATH="$HOME/.local/bin:$PATH"
tmd --version
```

## 用法

```sh
tmd README.md          # 渲染文件
cat notes.md | tmd     # 从 stdin 读
tmd --help             # 全部参数
```

支持标题、文字强调、列表、引用、链接、代码高亮和中英文表格。默认用颜色、粗细和分隔线区分标题层级。

静态文件在交互终端中默认从开头滚动阅读，使用当前主屏；滚轮或方向键上下阅读，Home/End 到首尾，q/Esc 返回命令行。非空文档保持打开，按 `/` 搜索正文，`n/N` 前后跳转，按 `o` 从标题目录跳转，按 `c` 选择代码块复制，按 `?` 查看帮助；空文档显示后直接返回。需要一次打印时使用 `tmd --ansi README.md`，管道及重定向行为不变。

默认无需配置。可通过 `--reader inline|native` 或可选 `tmd/config.toml` 的 `reader.mode` 选择阅读方式；Kaku 原生定位增强须额外配置适配脚本，缺失时回退内置阅读。配置、复制和退出说明见 [静态文件阅读](docs/reader.md)。

在 Kaku 中阅读静态 Markdown 文件时，可试用两档字号：顶层 H1 宽高各放大两倍，H2 至 H6 和正文维持常规字号。

```sh
tmd --large-headings fixtures/large-headings.md
```

该选项默认关闭，仅在直接连接 Kaku 的终端输出中生效；管道输入、重定向、`--ansi`、Claude JSONL、tmux/screen 和其他终端保留普通标题。窗口须至少 4 列、3 行，`--width` 须在 4 列至当前窗口宽度之间。放大的标题占两行，终端复制时可能得到重复文字；需要复制或保存时可去掉该选项。

```sh
tmd --theme neon-tokyo README.md
tmd --width 80 fixtures/acceptance.md
python3 scripts/slowcat.py fixtures/acceptance.md --rate 600 | tmd
```

内置主题为 `default-dark`、`default-light` 和 `neon-tokyo`，默认 `auto` 根据终端背景选择亮暗主题。自定义主题见 [主题说明](themes/README.md)。

## 当前限制

- 主要在 Kaku 中试用；Ghostty、kitty 的显示效果仍待补测。
- `--from claude-json` 已有真实样本回放测试，真实 Claude 增量管道仍待补测。
- 暂不支持图片显示；大标题尚不支持流式输出，也不提供逐级字号。
- 流式缩放只重排活动区，已进入终端历史的内容不重绘。

版本变化见 [CHANGELOG](CHANGELOG.md)，验收步骤见 [终端验收](docs/terminal-testing.md)。

维护者合并版本更新后，主分支 CI 全部通过会自动创建 tag 和 GitHub Release，具体流程见 [自动发布](docs/release.md)。

## 开发

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

项目保持 `tmd-core`（解析与布局）和 `tmd-cli`（输入与终端）两个 crate，详见[架构说明](docs/architecture.md)。验证命令和真实终端检查见[终端验收](docs/terminal-testing.md)，协作规则见 [AGENTS.md](AGENTS.md)。

仓库保留公开的产品、使用、架构和维护文档；计划、交接和讨论记录在仓库外管理。

## 许可证

MIT
