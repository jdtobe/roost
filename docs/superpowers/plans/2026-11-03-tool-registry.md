# Tool Registry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One definition per built-in tool, per-session tool options, and a tool list view, so adding a tool means one definition plus tests.

**Architecture:** A `Tool` trait in `roost-core` (`agent/tool/`) holds each tool's identity, prompt text, options and a `plan()` that returns data the chat loop acts on. `registry::specs()` is built from the tool list; `agent_loop.rs` stops matching on tool names. `EffectiveToolConfig` gains `settings`, applied on top of the app-wide `Limits` and clamped. The GUI edits options in the existing tool rows and adds a list window.

**Tech Stack:** Rust workspace (`roost-core`, `roost-gui` egui), serde_json, existing headless egui layout tests and fake-Ollama chat tests.

**Spec:** `docs/superpowers/specs/2026-11-02-roost-tool-registry-design.md` (approved 2026-11-03)

## Global Constraints

- Tool names, descriptions and parameter schemas stay byte-identical when ported (spec section 5). A golden test pins them (Task 1).
- No behaviour change in Tasks 1-5: every existing core, GUI and e2e test stays green at every commit, unmodified except for the code they call.
- `fetch_url` stays until `http_request` exists (spec 3.5). No aliases or id-stability work (spec 3.5).
- Per-session configuration only; no presets. Built-in tools only; no MCP.
- Settings switches (`agents_can_write`, `agents_can_run`) and app-wide `Limits` remain ceilings a session cannot exceed.
- Old session files load unchanged (`#[serde(default)]`).
- Gate before every push: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`, the same with `--features dev-hooks`, then `cargo test --workspace`.
- Tests first: commit the failing test, watch it fail, then implement. One branch per task off `main`, stacked only if a task needs the previous one unmerged.
- Edit files with Python or the Edit tool (macOS `sed -i` fails on multi-line patterns).

## Review Focus

- A saved session with no `settings` key loads with defaults and behaves as before (Task 6).
- A saved option of the wrong type, or out of range, is clamped or ignored, never a panic (Task 6).
- A session option above the app-wide ceiling is lowered to the ceiling (Task 6).
- A tool switched off while its approval card waits is refused when the user clicks Allow (Task 3).
- Two tools registered under one name, or a tool with an empty description or non-object schema, fails a test (Task 1).
- Editing an option mid-run does not change the call already running; the controls are disabled while a run is active (Tasks 6, 7).

## File Structure

- Create `crates/roost-core/src/agent/tool/mod.rs`: `Tool` trait, `Plan`, `Scope`, `OptionDecl`, `all()`.
- Create `agent/tool/files_read.rs`, `files_write.rs`, `shell.rs`, `ask.rs`, `web.rs`: one `impl Tool` per tool, delegating to the existing run code in `tools.rs`, `change.rs`, `command.rs`, `ask.rs`, `web.rs` (those files stay).
- Modify `agent/registry.rs`: `specs()` and `run()` use `tool::all()`; permission logic stays.
- Modify `chat/agent_loop.rs`: replace name matches with `plan()`.
- Modify `session.rs`: `EffectiveToolConfig.settings`.
- Modify `chat/types.rs`, `chat/manager.rs`: `ChatCommand::SetToolOption`.
- Modify GUI `chat/chrome.rs`, `chat/tool_limits.rs`, add `chat/tool_list.rs`; tests in `chat/layout_tests.rs`.
- Create `docs/ADDING_A_TOOL.md`.

---

### Task 1: The `Tool` trait, a golden snapshot, and `list_dir` + `read_file` ported

**Files:**
- Create: `crates/roost-core/src/agent/tool/mod.rs`, `files_read.rs`, `crates/roost-core/tests/golden/tool_definitions.json`
- Modify: `agent/mod.rs`, `agent/registry.rs`
- Test: `crates/roost-core/src/agent/registry.rs` (tests module)

**Interfaces:**
- Produces (used by Tasks 2-6):
  - `pub trait Tool: Send + Sync { fn spec(&self) -> ToolSpec; fn needs_workspace(&self) -> bool; fn options(&self) -> &'static [OptionDecl]; fn plan(&self, cx: &PlanCx, args: &Value) -> Plan; fn run(&self, cx: &RunCx, args: &Value) -> (Result<String, ToolError>, Option<change::Change>); }`
  - `pub enum Plan { Run, Approve { scope: Scope, preview: Option<Preview>, run: Option<RunPreview> }, Question, Refuse(ToolError) }` and `pub enum Scope { Capability, File(String), CommandPrefix(String), Host(String) }`. `Scope` is what "Allow for this chat" remembers and what an earlier allowance covers. Tasks 3-5 extend `Plan`/`Scope` if a real case needs it; Task 1 only has to cover reads.
  - `pub struct OptionDecl { pub key: &'static str, pub label: &'static str, pub help: &'static str, pub min: u64, pub max_from_limits: fn(&Limits) -> u64, pub get: fn(&Limits) -> u64, pub set: fn(&mut Limits, u64) }`
  - `pub struct PlanCx<'a> { pub workspace: Option<&'a Workspace>, pub limits: &'a Limits }`, `pub struct RunCx<'a> { pub workspace: &'a Workspace, pub limits: &'a Limits, pub opts: &'a Options<'a> }`
  - `pub fn all() -> &'static [&'static dyn Tool]`

- [ ] **Step 1: Write the golden test.** In `registry.rs` tests, `the_tool_definitions_sent_to_the_model_have_not_changed`: with every tool on (`EffectiveToolConfig::default()` plus capabilities all `Auto`), compare `ollama_definitions` pretty-printed to `tests/golden/tool_definitions.json`. Generate the file once from the current code (`UPDATE_GOLDEN=1` env switch in the test) and commit it in the same commit as the test. It passes now; it must pass unchanged through Task 5.
- [ ] **Step 2: Write the failing registry-shape test** `every_registered_tool_has_a_unique_name_a_description_and_an_object_schema` iterating `tool::all()`. Run: `cargo test -p roost-core registry::` Expected: compile error (`tool` module missing). Commit tests with an empty `tool/mod.rs` skeleton so the failure is the assertion, not the compiler.
- [ ] **Step 3: Implement the trait and types in `tool/mod.rs`; implement `ListDir` and `ReadFile` in `files_read.rs`** delegating to `tools::list_dir` / `tools::read_file`. `plan` returns `Plan::Run`, `needs_workspace` true. `all()` returns these two; `registry::specs()` returns `all()` specs for them followed by the existing spec list for the rest (temporary). `registry::run` asks the tool first, then falls back to the old match.
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: all pass including golden and shape tests.
- [ ] **Step 5: Commit** `feat(agent): Tool trait; list_dir and read_file defined through it`.

### Task 2: Port the remaining read tools

**Files:** Modify `agent/tool/files_read.rs`, `agent/registry.rs`.

**Interfaces:** Consumes `Tool`, `Plan::Run` (Task 1). Produces `ReadBinary`, `FindFiles`, `SearchText`, `SearchBytes` in `all()`.

- [ ] **Step 1: Test** `registry::every_read_tool_is_listed_once_in_all` asserts `all()` contains `read_binary`, `find_files`, `search_text`, `search_bytes` and that the temporary fallback list in `registry::specs()` no longer holds them. Run, expect FAIL.
- [ ] **Step 2: Implement** the four tools as in Task 1; move their specs (text copied verbatim) out of `registry::specs()`.
- [ ] **Step 3: Run** `cargo test --workspace` (golden unchanged). Expected: PASS.
- [ ] **Step 4: Commit** `refactor(agent): the other read tools through the Tool trait`.

### Task 3: Write tools and the approval plan

**Files:** Create `agent/tool/files_write.rs`. Modify `agent/registry.rs`, `chat/agent_loop.rs` (`run_next_tool`, `resolve_approval`, `preview`, `run_preview`).

**Interfaces:**
- Consumes: `Plan::Approve`, `Scope::File(String)`.
- Produces: `WriteFile`, `EditFile` whose `plan()` returns `Plan::Approve { scope: Scope::File(path), preview: Some(..), run: Some(..) }` for a plannable call and `Plan::Run` for one that cannot be planned (so the model reads why). The loop treats `live.allowed_files.contains(path)` as "already allowed". `AllowForSession` on a `Scope::File` adds the path instead of raising the capability.

- [ ] **Step 1: Tests (all in `chat/tests.rs`, written against existing behaviour so they pass before the port, then guard it):** `an_unplannable_write_runs_without_asking`, `allow_for_session_on_a_write_allows_only_that_file`, and the Review Focus test `a_tool_switched_off_while_its_approval_waits_is_refused_on_allow`. Run first: the first two must pass on current code (they pin behaviour); the third is new, expect FAIL if the loop does not re-check; if it passes already, keep it and say so in the commit.
- [ ] **Step 2: Implement** `WriteFile`/`EditFile` using `change::plan`/`apply` (move `registry::write` here). In `run_next_tool` replace the `"files.write"` capability branch with a match on the tool's `Plan`; in `resolve_approval` replace the `"files.write"` arm with a match on `Scope`.
- [ ] **Step 3: Run** `cargo test --workspace`. Expected: PASS, golden unchanged.
- [ ] **Step 4: Commit** `refactor(agent): write tools and their approval through Plan`.

### Task 4: `run_command`

**Files:** Create `agent/tool/shell.rs`. Modify `agent/registry.rs`, `chat/agent_loop.rs`.

**Interfaces:** `RunCommand::plan` returns `Plan::Approve { scope: Scope::CommandPrefix(prefix), preview: None, run: Some(..) }`; a covered command (`command::covered_by` against `live.allowed_prefixes`) or a call without a command skips approval. `AllowForSession` on `CommandPrefix` extends `allowed_prefixes` (via `command::allow_prefix`); an unsafe prefix allows only this call.

- [ ] **Step 1: Tests** `an_allowed_prefix_covers_a_plain_command_but_not_a_chained_one`, `a_command_with_no_command_argument_runs_to_fail_without_asking` in `chat/tests.rs` (pin current behaviour). Run: expect PASS on current code.
- [ ] **Step 2: Implement** `RunCommand`; remove the `"run_command"` special case in `run_next_tool` and the `shell.run` arm in `resolve_approval`.
- [ ] **Step 3: Run** `cargo test --workspace` and `grep -n '"run_command"' crates/roost-core/src/chat/agent_loop.rs`. Expected: tests PASS, grep empty.
- [ ] **Step 4: Commit** `refactor(agent): run_command through Plan`.

### Task 5: `ask_user` and `fetch_url`; the loop matches no names

**Files:** Create `agent/tool/ask.rs`, `agent/tool/web.rs`. Modify `agent/registry.rs`, `chat/agent_loop.rs`.

**Interfaces:** `AskUser::plan` returns `Plan::Question` (parse errors via `Plan::Refuse`). `FetchUrl::plan` returns `Plan::Approve { scope: Scope::Host(host), .. }`, or `Plan::Refuse` when no usable address. Add `fn exec(&self) -> Exec` to the trait with `enum Exec { Blocking, Network }`; the loop spawns the network path for `Network` (replacing `name == web::TOOL`).

- [ ] **Step 1: Tests** `an_allowed_host_is_fetched_without_asking`, `a_fetch_without_an_address_fails_and_the_model_reads_why`, `the_web_never_becomes_automatic_on_allow_for_session` (pin current behaviour). Run: expect PASS.
- [ ] **Step 2: Implement** both; remove `ask::TOOL`/`web::TOOL` comparisons from the loop, `resolve_approval`, `ask_question`'s `specs().find` lookup.
- [ ] **Step 3: Run** `cargo test --workspace` and `grep -nE '"(list_dir|read_file|read_binary|find_files|search_text|search_bytes|write_file|edit_file|run_command)"|ask::TOOL|web::TOOL' crates/roost-core/src/chat/agent_loop.rs`. Expected: tests PASS, grep empty; the temporary fallback in `registry::run` and `specs()` is deleted.
- [ ] **Step 4: Commit** `refactor(agent): the loop asks each tool for its plan`.

### Task 6: Per-session options in core

**Files:** Modify `session.rs`, `agent/tool/*.rs` (declare options), `agent/tools.rs` (`Limits`), `chat/types.rs`, `chat/manager.rs`, `chat/agent_loop.rs` (`limits_for`).

**Interfaces:**
- Produces: `EffectiveToolConfig.settings: BTreeMap<String, serde_json::Value>`; `pub fn effective_limits(base: &Limits, config: &EffectiveToolConfig) -> Limits` in `registry.rs`; `ChatCommand::SetToolOption { session: SessionId, tool: String, option: String, value: Option<u64> }` (`None` clears the override); the existing `ChatEvent::AgentSettings` carries the result. Options declared first: `list_dir.limit` (list_entries), `read_file.default_lines`/`max_lines`, `find_files.results`, `search_text.max_matches`/`per_file_matches`, `run_command.timeout_secs`, `run_command.output_bytes`, each `min` 1.
- Rule: effective = session override, else app-wide value, else default; always `min..=base value` (the app-wide value is the ceiling) then cut to the model window as `Limits::for_window` already does.

- [ ] **Step 1: Tests (core).** In `session.rs`: `a_session_file_without_settings_loads_with_none`. In `registry.rs`: `an_override_below_the_ceiling_is_used`, `an_override_above_the_ceiling_is_lowered_to_it` , `an_override_of_the_wrong_type_is_ignored`, `an_override_below_the_minimum_is_raised_to_it`, `an_unknown_tool_or_option_is_ignored`. In `chat/tests.rs`: `setting_a_tool_option_saves_it_and_emits_agent_settings`, `a_call_already_running_keeps_the_limits_it_started_with`. Run: compile error then FAIL.
- [ ] **Step 2: Implement** `settings` (serde default), `OptionDecl` lists on the tools above, `effective_limits`, the command, and `limits_for` applying `effective_limits` before `for_window`. Setting an option while a run is active is accepted but applies from the next call.
- [ ] **Step 3: Run** `cargo test --workspace`. Expected: PASS.
- [ ] **Step 4: Mutation check:** make `effective_limits` return `base.clone()`; confirm the override tests fail; restore.
- [ ] **Step 5: Commit** `feat(agent): per-session tool options, clamped to the app-wide ceilings`.

### Task 7: Editable options in the tool rows

**Files:** Modify `roost-gui/src/chat/chrome.rs` (`tool_limit_chip`, `tool_limit_panel`), `chat/tool_limits.rs`, `chat/state.rs` if the open session's `settings` need folding; tests in `chat/layout_tests.rs`.

**Interfaces:** Consumes `OptionDecl`, `SetToolOption`. The open tool row lists each declared option as a drag value from `min` to the app-wide ceiling; changing it sends `SetToolOption`, a "Reset" per option sends `value: None`; "Change in Settings" stays for the ceilings. Controls are disabled while the run is active.

- [ ] **Step 1: Tests** `a_tool_row_shows_its_options_and_changing_one_sends_it` (assert the sent `SetToolOption`), `reset_clears_the_override`, `the_option_controls_are_disabled_while_a_run_is_active`, each at every size in `SIZES`, with all rectangles inside the popover. Run: expect FAIL.
- [ ] **Step 2: Implement** the rows from `OptionDecl`, keeping `tool_limits::summary` wording where existing tests pin it.
- [ ] **Step 3: Run** `cargo test -p roost-gui`. Expected: PASS.
- [ ] **Step 4: Commit** `feat(gui): edit a tool's options per session`.

### Task 8: The tool list window

**Files:** Create `roost-gui/src/chat/tool_list.rs`. Modify `chat/chrome.rs` (an "All tools…" button in the tools popover), `chat/view.rs` (`ChatUi.tool_list_open: bool`, probe fields), `chat/layout_tests.rs`.

**Interfaces:** An `egui::Window` listing every tool grouped by family: name, one-line description, status for this session (off / ask / allowed), its options (the Task 7 rows) and the model-facing description in a collapsible. Reads the same session config as the popover; no new data. Available only when the tools gate is `Ready`.

- [ ] **Step 1: Tests** `the_tool_list_shows_every_tool_grouped_by_family`, `a_tool_row_in_the_list_toggles_the_tool`, `the_tool_list_is_inside_the_window_at_every_size`. Run: expect FAIL.
- [ ] **Step 2: Implement** the window; the toggle sends the existing `SetToolEnabled`-style command the popover uses.
- [ ] **Step 3: Run** the full gate. Expected: PASS.
- [ ] **Step 4: Commit** `feat(gui): a list of every tool and its settings`.

### Task 9: Docs and a real-model check

**Files:** Create `docs/ADDING_A_TOOL.md`. Modify `CHANGELOG.md`, `docs/ROADMAP.md`, `docs/superpowers/notes/handoff-2026-10-29.md`.

- [ ] **Step 1:** Write `ADDING_A_TOOL.md`: the file to create, the trait methods, `all()` registration, the golden file to regenerate, the tests a new tool needs.
- [ ] **Step 2:** Update the changelog, roadmap and handoff in the same commit as the last code change they describe.
- [ ] **Step 3 (needs the user's go):** Run the hand-run harness against the throwaway Ollama on port 11555 (`crates/roost-core/tests/hand_run_agent.rs`, ignored tests) to confirm byte-identical prompts still behave. Report the result, including failures.
- [ ] **Step 4:** Run the full gate; push; open the PR.

---

## Self-Review

- **Spec coverage:** 3.1 trait (Tasks 1-5), 3.2 settings (6), 3.3 popover, Settings ceilings and list view (7, 8), 3.4 phase 1 only (all built-in), decisions 3.5 (Global Constraints), order of work (task order), risks (golden test, one tool per step, harness in Task 9), docs (9).
- **Types:** `Plan`, `Scope`, `OptionDecl`, `PlanCx`, `RunCx`, `Exec` are defined in Task 1/5 and used with the same names later; `SetToolOption` defined in Task 6, used in Tasks 7-8.
- **Known soft spot:** `Plan`/`Scope` are designed from reading the loop, not from compiling against it; Task 3 may need to widen them. That is why Task 1 ports only reads and Tasks 3-5 each port one special case.
- **Decision for the user:** Task 8 shows the tool list as a window opened from the tools popover, not a new top-level page. Say if you want a page.
