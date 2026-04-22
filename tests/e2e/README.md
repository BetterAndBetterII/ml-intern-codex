# E2E checklist

Manual validation targets from `docs/TEST_PLAN.md`:

1. `ml-intern` 在真实 TTY 中默认进入全屏 `Session / Transcript / Composer` 布局，并在右下 status toast 看到 ready 提示；`~/.ml-intern-codex/config.toml` 和 `~/.ml-intern-codex/db/state.sqlite` 会在首启后存在。非 TTY 或 `--line-mode` 仍保留旧 line-mode 作为联调 / deterministic smoke fallback。
2. First prompt creates a thread and streams assistant output；这些 delta / `commandExecution` / `fileChange` / `plan` / `artifact` cells 应在 turn 进行中实时出现，而不是等 turn 结束后一次性刷出来；若命令执行较长，`item/commandExecution/outputDelta` 应持续把 stdout/stderr 追加进同一个 `exec~` transcript cell，而不是只在 completed 时才看到整段输出。
3. 输入 `$`（或 `/skills`）会打开真正的 skill picker overlay，可直接在 overlay 内 filter bundled/user/repo skills，并显示 scope + path；重复技能名时仍能在发送前选中正确目标。
4. `/threads` opens the thread picker overlay、支持 filter 后恢复目标 thread；如果上游 resume 失败，客户端仍应回落到本地 transcript/turn 快照并给出明确 warning，而不是整条线程不可见。
5. Resume one thread that contains approvals/artifacts/warnings；可额外插入一条损坏的 transcript JSON 行，确认 replay 会跳过坏行、给出 warning，并继续恢复其余 cells；若 transcript 里已有长命令的 `command_execution_output_delta` 片段，恢复后也应能看到累积后的 `exec<` 输出；若该线程仍处于 Running / WaitingApproval，也要确认前台会继续消费通知并允许 interrupt / approval。
6. Trigger one approval and confirm the client automatically opens approval overlay、允许 approve/reject 并在关闭后恢复 turn；如果是 file-change approval，approval 前的 transcript 里应该已经能看到 `patch>` 摘要和目标文件；若 `approval/respond` 故意失败，客户端应保留 pending approval、给出 error，并且 error/status toast 不能被 approval modal 完全遮住，仍允许用 `/approval` 重新打开 overlay 重试。
7. Trigger one `request_user_input` prompt and confirm the questionnaire overlay can collect answers / option selection / free-form input, keep `isSecret` answers masked on screen, then forward the structured answers.
8. During a long-running turn, press `Esc` (or `Ctrl+C`) and confirm the client sends `turn/interrupt` and the thread returns to ready state（当前 deterministic PTY smoke 已覆盖默认 full-screen `Esc` interrupt、help overlay 打开时 `Ctrl+C` 仍优先 interrupt，以及 interrupt RPC 失败后仍保留 busy gate / error 提示）；若 interrupt RPC 故意失败，客户端应保留 live turn 并显示 error，而不是直接退出前台会话。
9. During that same long-running turn, try `/threads`、`/skills`、`/artifacts`、`/clear` 或空 composer 下的 `$` 快捷键；客户端应提示先 interrupt 当前 turn，而不是在 live session 底下偷偷打开 picker / 清空 transcript。若正处于 pending approval，则这些入口应改为提示先用 `/approval` 处理。
10. During a long-running artifact-producing turn, confirm a new artifact cell can appear before `turn/completed`，并且摘要会带上对应 flow 的关键字段（dataset/splits/issues、query/paper_count/recipe、job/status/hardware/url）。
11. Drop or simulate one malformed `artifact.json` and confirm the client shows a warning instead of crashing the session.
12. Run one helper smoke test such as `PYTHONPATH=helpers/python/src python3 -m mli_helpers.artifacts.write_dataset_audit --artifact-dir <cwd>/.ml-intern/threads/<thread-id>/artifacts/<artifact-id> --dataset demo --split train` and confirm the files land in the expected schema.
13. `/artifacts` opens the artifact list overlay、支持 filter 后打开目标 payload；artifact viewer overlay 能在同一个 artifact 内切换 markdown / json / text 文件，并在缺失文件时显示明确 read error（当前 deterministic PTY smoke 已覆盖缺失 `raw.txt` 的回归）。
14. `ml-intern-app-server` speaks JSONL for `initialize`, `thread/*`, `turn/*`, `approval/respond`, `skills/list`, `artifact/*`.
