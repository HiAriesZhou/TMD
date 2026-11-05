#!/usr/bin/env python3
"""Exercise the real static-reader process, terminal events and restoration.

This is protocol evidence; Kaku visual acceptance is recorded separately.
"""
import argparse
import base64
import errno
import fcntl
import os
from pathlib import Path
import pty
import re
import select
import signal
import struct
import sys
import tempfile
import termios
import time
import unittest

BINARY = Path(__file__).resolve().parents[1] / "target/debug/tmd"
END = b"\x1b[?2026l"


class Terminal:
    def __init__(self, options=(), *, row=12, config=None, native=False, capacity=10000,
                 wrong_token=False, text=None, answer_cursor=True, interrupt_probe=False,
                 cursor_prefix=b""):
        self.temp = tempfile.TemporaryDirectory(prefix="tmd-reader-")
        root = Path(self.temp.name)
        path = root / "article.md"
        path.write_text(text if text is not None else
                        "# Heading 中 é 👩‍💻\n\n" +
                        "\n\n".join(f"Row {i:03} 中文" for i in range(1, 81)))
        if config is not None:
            (root / "tmd").mkdir()
            (root / "tmd/config.toml").write_text(config)
        ready_read, ready_write = os.pipe()
        self.pid, self.master = pty.fork()
        if self.pid == 0:
            os.close(ready_write)
            os.read(ready_read, 1)
            os.close(ready_read)
            env = dict(os.environ, TERM="xterm-256color", TERM_PROGRAM="Kaku",
                       XDG_CONFIG_HOME=str(root))
            for key in ("TMUX", "STY"):
                env.pop(key, None)
            args = [str(BINARY), "--theme", "default-dark", *options, str(path)]
            os.execve(str(BINARY), args, env)
        os.close(ready_read)
        self.resize(80, 24)
        self.before = termios.tcgetattr(self.master)
        self.data = bytearray()
        self.cursor = row
        self.cursor_prefix = cursor_prefix
        self.native = native
        self.capacity = capacity
        self.wrong_token = wrong_token
        self.interrupt_probe = interrupt_probe
        self.answer_cursor = answer_cursor
        self.replied = False
        self.waited = False
        os.write(ready_write, b"1")
        os.close(ready_write)

    def resize(self, cols, rows):
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

    def read(self):
        if not select.select([self.master], [], [], .05)[0]:
            return True
        try:
            chunk = os.read(self.master, 65536)
        except OSError as error:
            if error.errno == errno.EIO:
                return False
            raise
        if not chunk:
            return False
        self.data.extend(chunk)
        if self.interrupt_probe and b"SetUserVar=TMD_READER=" in self.data:
            os.write(self.master, b"\x03")
            self.interrupt_probe = False
        if self.answer_cursor and b"\x1b[6n" in self.data and not self.replied:
            os.write(self.master, self.cursor_prefix + f"\x1b[{self.cursor};1R".encode())
            self.replied = True
        if self.native:
            match = re.search(rb"SetUserVar=TMD_READER=([A-Za-z0-9+/=]+)\x07", self.data)
            if match:
                _, action, token, path = base64.b64decode(match[1]).decode().split(":", 3)
                if action == "probe":
                    reply = Path(path)
                    try:
                        assert reply.read_text() == f"tmd-reader-v1:{token}\n"
                        assert reply.parent.stat().st_mode & 0o777 == 0o700
                        if self.wrong_token:
                            reply.write_text("tmd-reader-v1:other\n")
                        with reply.open("a") as output:
                            output.write(f"{self.capacity}\n")
                    except FileNotFoundError:
                        # Cancellation can remove the request before this
                        # simulated callback runs, just as in the Lua adapter.
                        pass
                    self.native = False
        return True

    def until(self, marker, offset=0):
        deadline = time.monotonic() + 8
        while marker not in self.data[offset:]:
            if time.monotonic() > deadline or not self.read():
                raise AssertionError(f"Missing {marker!r}; tail={bytes(self.data[-400:])!r}")
        return bytes(self.data[offset:])

    def frame(self, key=None):
        offset = len(self.data)
        if key:
            os.write(self.master, key)
        return self.until(END, offset)

    def finish(self, key=None):
        if key:
            os.write(self.master, key)
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            if not self.read():
                break
        else:
            raise AssertionError("reader failed to exit")
        _, status = os.waitpid(self.pid, 0)
        self.waited = True
        return os.waitstatus_to_exitcode(status)

    def close(self):
        if not self.waited:
            # Release a child blocked in terminal output before waiting for it.
            os.close(self.master)
            self.master = None
            try:
                os.kill(self.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline:
                pid, _ = os.waitpid(self.pid, os.WNOHANG)
                if pid:
                    self.waited = True
                    break
                time.sleep(.01)
            if not self.waited:
                raise RuntimeError(f"test child {self.pid} did not reap after SIGKILL")
        if self.master is not None:
            os.close(self.master)
        self.temp.cleanup()


class ReaderTests(unittest.TestCase):
    def terminal(self, *args, **kwargs):
        terminal = Terminal(*args, **kwargs)
        self.addCleanup(terminal.close)
        return terminal

    def assert_restored(self, terminal):
        self.assertEqual(termios.tcgetattr(terminal.master), terminal.before)
        self.assertNotIn(b"?1049", terminal.data)
        self.assertNotIn(b"\x1b[3J", terminal.data)

    def test_open_at_top_then_wheel_and_end_reach_pre_rendered_tail(self):
        for row in (1, 12, 24):
            with self.subTest(row=row):
                t = self.terminal(("--large-headings",), row=row)
                first = t.until(END)
                self.assertIn(b"Row 001", first)
                self.assertNotIn(b"Row 080", first)
                self.assertIn(b"\x1b[1;1H", first)
                self.assertEqual(first.count(b"\x1b#3"), first.count(b"\x1b#4"))
                wheel = t.frame(b"\x1b[<65;5;5M")
                self.assertNotEqual(first, wheel)
                last = t.frame(b"\x1b[F")
                self.assertIn(b"Row 080", last)
                top = t.frame(b"\x1b[H")
                self.assertIn(b"Row 001", top)
                self.assertEqual(t.finish(b"q"), 0)
                self.assert_restored(t)

    def test_resize_including_tiny_window_keeps_exit_available(self):
        t = self.terminal(("--large-headings",))
        t.until(END)
        offset = len(t.data)
        t.resize(20, 8)
        t.until(END, offset)
        offset = len(t.data)
        t.resize(4, 1)
        frame = t.until(END, offset)
        self.assertNotIn(b"\x1b#3", frame)
        self.assertEqual(t.finish(b"\x1b"), 0)
        self.assert_restored(t)

    def test_help_preserves_position_and_paste_does_not_execute_commands(self):
        t = self.terminal()
        t.until(END)
        tail = t.frame(b"\x1b[F")
        self.assertIn(b"Row 080", tail)
        self.assertIn("阅读帮助".encode(), t.frame(b"?"))
        self.assertIn(b"Row 080", t.frame(b"\x1b"))
        # Bracketed paste is a single Paste event, never a sequence of keys.
        self.assertIn(b"Row 080", t.frame(b"\x1b[200~q?jk\x1b[201~"))
        self.assertIn("阅读帮助".encode(), t.frame(b"?"))
        self.assertIn(b"Row 080", t.frame(b"q"))
        self.assertEqual(t.finish(b"q"), 0)
        self.assertIn(b"\x1b[?2004h", t.data)
        self.assertIn(b"\x1b[?2004l", t.data)
        self.assert_restored(t)

    def test_growing_window_does_not_exit_short_document(self):
        t = self.terminal(text="# Short\n\nBody")
        t.until(END)
        offset = len(t.data)
        t.resize(120, 50)
        self.assertIn(b"Body", t.until(END, offset))
        self.assertIn("阅读帮助".encode(), t.frame(b"?"))
        self.assertEqual(t.finish(b"\x03"), 130)
        self.assert_restored(t)

    def test_keyboard_ctrl_c_and_external_signals_restore_termios(self):
        for signum in (None, signal.SIGINT, signal.SIGTERM, signal.SIGHUP, signal.SIGQUIT):
            with self.subTest(signal=signum):
                t = self.terminal()
                t.until(END)
                if signum:
                    os.kill(t.pid, signum)
                self.assertEqual(t.finish(None if signum else b"\x03"), 128 + (signum or signal.SIGINT))
                self.assert_restored(t)

    def test_native_probe_precedes_full_output_and_emits_show_once(self):
        t = self.terminal(config="[reader]\nmode='native'", native=True)
        self.assertEqual(t.finish(), 0)
        self.assertIn(b"Row 080", t.data)
        self.assertNotIn(b"?1000h", t.data)
        messages = re.findall(rb"SetUserVar=TMD_READER=([A-Za-z0-9+/=]+)\x07", t.data)
        actions = [base64.b64decode(value).decode().split(":")[1] for value in messages]
        self.assertEqual(actions, ["probe", "show"])
        self.assert_restored(t)

    def test_missing_wrong_or_small_native_adapter_falls_back_before_tail(self):
        for kwargs in ({}, {"native": True, "wrong_token": True}, {"native": True, "capacity": 1}):
            with self.subTest(kwargs=kwargs):
                t = self.terminal(("--reader", "native"), **kwargs)
                first = t.until(END)
                self.assertIn(b"Row 001", first)
                self.assertNotIn(b"Row 080", first)
                self.assertEqual(t.finish(b"q"), 0)
                self.assert_restored(t)

    def test_short_empty_and_cursor_query_failure_restore_terminal(self):
        t = self.terminal(text="")
        self.assertEqual(t.finish(), 0)
        self.assert_restored(t)
        t = self.terminal(text="# Short\n\nBody")
        t.until(END)
        self.assertIn("阅读帮助".encode(), t.frame(b"?"))
        self.assertIn(b"Body", t.frame(b"q"))
        self.assertEqual(t.finish(b"q"), 0)
        self.assert_restored(t)
        t = self.terminal(answer_cursor=False)
        self.assertEqual(t.finish(), 1)
        self.assert_restored(t)

    def test_empty_document_preserves_interrupt_during_cursor_query(self):
        t = self.terminal(text="", cursor_prefix=b"\x03")
        self.assertEqual(t.finish(), 130)
        self.assert_restored(t)

    def test_visible_text_search_counts_navigation_and_cancel(self):
        t = self.terminal(text="\n\n".join(f"Paragraph {i}: needle 中文" for i in range(40)))
        t.until(END)
        t.frame(b"/")
        t.frame(b"\x1b[200~needle\x1b[201~")
        result = t.frame(b"\r")
        self.assertIn("匹配 1/40".encode(), result)
        self.assertIn("匹配 2/40".encode(), t.frame(b"n"))
        self.assertIn("匹配 1/40".encode(), t.frame(b"N"))
        t.frame(b"/")
        # Commands in a query are literal, and cancelling restores the query.
        t.frame(b"\x1b[200~qjknc\x1b[201~")
        self.assertIn("匹配 1/40".encode(), t.frame(b"\x1b"))
        t.frame(b"/")
        t.frame(b"\x15")  # Ctrl-U clears the edit buffer.
        t.frame(b"\x1b[200~not found\x1b[201~")
        self.assertIn("未找到匹配".encode(), t.frame(b"\r"))
        t.frame(b"\x1b")  # Clear search, stay in reader.
        self.assertEqual(t.finish(b"q"), 0)
        self.assert_restored(t)

    def test_search_input_resize_cancel_and_interrupt_restore_terminal(self):
        t = self.terminal()
        t.until(END)
        t.frame(b"\x1b[F")
        t.frame(b"/")
        t.frame(b"\x1b[200~" + "中文 👩‍💻".encode() + b"\x1b[201~")
        offset = len(t.data)
        t.resize(120, 50)
        t.until(END, offset)
        self.assertIn(b"Row 080", t.frame(b"\x1b"))
        t.frame(b"/")
        self.assertEqual(t.finish(b"\x03"), 130)
        self.assert_restored(t)

    def test_code_chooser_cancel_and_paste_preserve_reading(self):
        text = "# Codes\n\n```rust\nfirst()\n```\n\n" + "\n\n".join(f"Paragraph {i}" for i in range(30)) + "\n\n```sh\nsecond\n```"
        t = self.terminal(text=text)
        t.until(END)
        t.frame(b"\x1b[F")
        menu = t.frame(b"c")
        self.assertIn("代码块".encode(), menu)
        self.assertIn(b"2/2", menu)
        self.assertIn(b"rust", menu)
        self.assertIn(b"sh", menu)
        # The payload's Enter/q/c must never be executed as chooser actions.
        self.assertIn("代码块".encode(), t.frame(b"\x1b[200~\rqcc\x1b[201~"))
        returned = t.frame(b"\x1b")
        self.assertIn(b"second", returned)
        self.assertEqual(t.finish(b"q"), 0)
        self.assert_restored(t)

    def test_code_chooser_resize_and_ctrl_c_restore_terminal(self):
        t = self.terminal(text="```\ncode\n```")
        t.until(END)
        t.frame(b"c")
        offset = len(t.data)
        t.resize(4, 1)
        t.until(END, offset)
        self.assertEqual(t.finish(b"\x03"), 130)
        self.assert_restored(t)
        t = self.terminal(text="no code blocks here")
        t.until(END)
        self.assertIn("未找到代码块".encode(), t.frame(b"c"))
        self.assertEqual(t.finish(b"q"), 0)
        self.assert_restored(t)

    def test_code_chooser_defaults_to_visible_empty_block(self):
        text = "```first\none\n```\n\n```empty\n```\n\n" + "\n\n".join(
            f"After {i:02}" for i in range(30)) + "\n\n```last\nthree\n```"
        t = self.terminal(text=text)
        t.until(END)
        for _ in range(3):
            t.frame(b"j")
        self.assertIn(b"2/3", t.frame(b"c"))
        # No Enter: this regression never touches the real clipboard.
        self.assertIn(b"After 00", t.frame(b"\x1b"))
        self.assertEqual(t.finish(b"q"), 0)
        self.assert_restored(t)

    def test_code_chooser_selects_next_code_after_decorative_viewport(self):
        text = "```\none\n```\n\n" + "---\n\n" * 30 + "```\nthree\n```"
        t = self.terminal(text=text)
        t.until(END)
        for _ in range(4):
            frame = t.frame(b"j")
        self.assertNotIn(b"one", frame)
        self.assertNotIn(b"three", frame)
        self.assertIn(b"2/2", t.frame(b"c"))
        t.frame(b"\x1b")
        self.assertEqual(t.finish(b"q"), 0)
        self.assert_restored(t)

    def test_outline_lists_top_level_headings_and_jumps_to_duplicate(self):
        paragraphs = lambda prefix: "\n\n".join(f"{prefix} {i:02}" for i in range(30))
        text = ("# Same\n\n" + paragraphs("Before") +
                "\n\n> # quote-example\n\n- # list-example\n\n```md\n# fenced-example\n```\n\n" +
                "# Same\n\n" + paragraphs("After") + "\n\nSetext\n------\n\n" + paragraphs("Tail"))
        t = self.terminal(text=text)
        self.assertIn(b"Before 00", t.until(END))
        menu = t.frame(b"o")
        self.assertIn("标题目录".encode(), menu)
        self.assertIn(b"1/3", menu)
        self.assertEqual(menu.count(b"H1 Same"), 2)
        self.assertIn(b"H2 Setext", menu)
        for excluded in (b"quote-example", b"list-example", b"fenced-example"):
            self.assertNotIn(excluded, menu)
        t.frame(b"\x1b[B")
        jumped = t.frame(b"\r")
        self.assertIn(b"After 00", jumped)
        self.assertNotIn(b"Before 00", jumped)
        self.assertIn(b"2/3", t.frame(b"o"))
        self.assertIn("标题目录".encode(), t.frame(b"\x1b[200~\rqo\x1b[201~"))
        self.assertIn(b"After 00", t.frame(b"q"))
        self.assertEqual(t.finish(b"q"), 0)
        self.assert_restored(t)

    def test_outline_resize_cancel_preserves_search_and_large_heading(self):
        text = "# Top\n\n" + "\n\n".join(f"Before {i:02}" for i in range(30))
        text += "\n\n# Target 中文\n\n" + "\n\n".join(f"After {i:02}" for i in range(30))
        t = self.terminal(("--large-headings",), text=text)
        t.until(END)
        t.frame(b"/")
        # One paste event avoids mistaking a partial per-key frame for the
        # completed query when the PTY batches several printable keys.
        t.frame(b"\x1b[200~Target\x1b[201~")
        self.assertIn("匹配 1/1".encode(), t.frame(b"\r"))
        t.frame(b"o")
        t.frame(b"\x1b[H")
        offset = len(t.data)
        t.resize(20, 8)
        t.until(END, offset)
        returned = t.frame(b"\x1b")
        self.assertIn(b"Target", returned)
        self.assertEqual(returned.count(b"\x1b#3"), returned.count(b"\x1b#4"))
        offset = len(t.data)
        t.resize(80, 24)
        self.assertIn("匹配 1/1".encode(), t.until(END, offset))
        self.assertIn(b"2/2", t.frame(b"o"))
        self.assertIn(b"After 00", t.frame(b"\r"))
        self.assertEqual(t.finish(b"\x03"), 130)
        self.assert_restored(t)

    def test_outline_no_headings_and_tiny_window_interrupt(self):
        t = self.terminal(text="plain document")
        t.until(END)
        self.assertIn("未找到顶层标题".encode(), t.frame(b"o"))
        self.assertEqual(t.finish(b"q"), 0)
        self.assert_restored(t)
        t = self.terminal(text="# Heading\n\nbody")
        t.until(END)
        t.frame(b"o")
        offset = len(t.data)
        t.resize(4, 1)
        t.until(END, offset)
        self.assertEqual(t.finish(b"\x03"), 130)
        self.assert_restored(t)

    def test_outline_confirm_before_or_during_tiny_window_keeps_target(self):
        text = "# First\n\n" + "\n\n".join(f"Before {i:02}" for i in range(30))
        text += "\n\n# Target\n\n" + "\n\n".join(f"After {i:02}" for i in range(30))
        for confirm_while_tiny in (True, False):
            with self.subTest(confirm_while_tiny=confirm_while_tiny):
                t = Terminal(("--large-headings",), text=text)
                try:
                    self.assertIn(b"Before 00", t.until(END))
                    t.frame(b"o")
                    t.frame(b"\x1b[B")
                    if not confirm_while_tiny:
                        self.assertIn(b"After 00", t.frame(b"\r"))
                    offset = len(t.data)
                    t.resize(4, 1)
                    t.until(END, offset)
                    if confirm_while_tiny:
                        t.frame(b"\r")
                    offset = len(t.data)
                    t.resize(80, 24)
                    returned = t.until(END, offset)
                    self.assertIn(b"Target", returned)
                    self.assertIn(b"After 00", returned)
                    self.assertNotIn(b"Before 00", returned)
                    self.assertEqual(returned.count(b"\x1b#3"), returned.count(b"\x1b#4"))
                    self.assertEqual(t.finish(b"q"), 0)
                    self.assert_restored(t)
                finally:
                    t.close()

    def test_cli_override_and_invalid_config(self):
        t = self.terminal(("--reader", "inline"), config="[reader]\nmode='native'")
        first = t.until(END)
        self.assertNotIn(b"SetUserVar", first)
        self.assertEqual(t.finish(b"q"), 0)
        t = self.terminal(config="[reader]\nmode='typo'")
        self.assertEqual(t.finish(), 1)
        self.assertIn(b"reader.mode", t.data)
        self.assert_restored(t)

    def test_ctrl_c_during_native_probe_wins_over_short_document_exit(self):
        for native in (False, True):
            t = self.terminal(("--reader", "native"), text="# Short", interrupt_probe=True, native=native)
            self.assertEqual(t.finish(), 130)
            self.assert_restored(t)

    def test_timed_out_probe_removes_reply_channel_before_returning_to_shell(self):
        t = self.terminal(("--reader", "native"), text="# Short")
        t.until(END)
        self.assertEqual(t.finish(b"q"), 0)
        match = re.search(rb"SetUserVar=TMD_READER=([A-Za-z0-9+/=]+)\x07", t.data)
        version, action, token, path = base64.b64decode(match[1]).decode().split(":", 3)
        self.assertEqual((version, action), ("v1", "probe"))
        reply = Path(path)
        self.assertFalse(reply.parent.exists())
        # The adapter requires an existing marker and parent. A delayed
        # callback cannot write a reply or inject anything into terminal input.
        with self.assertRaises(FileNotFoundError):
            reply.open("a")
        self.assertNotIn(b"\x1b[200~", t.data)
        self.assert_restored(t)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=BINARY)
    args = parser.parse_args()
    BINARY = args.binary.resolve()
    unittest.main(argv=[sys.argv[0]])
