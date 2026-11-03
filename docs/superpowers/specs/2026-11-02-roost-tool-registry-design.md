# A tool registry that agents can configure and that can grow (DRAFT)

Status: **draft for the user's review, written while they were away (2026-11-02).** Nothing here is built. Sections marked **Decision needed** are the things only the user can settle; everything else is a recommendation with a reason. After review this goes to an implementation plan.

## 1. Intent

The user wants (their words, 2026-11-02): "a set of tools that can be dynamically managed on/off or configured for each agent", "a comprehensive tool list", and "extensibility for what it's inbuilt". Swarms, the NAS idea and the http_request tool are parked; this work should make adding any later tool cheap.

Success looks like: one place that says what a tool is; every tool switchable and configurable per agent from the UI; adding a built-in tool means writing one tool definition and its tests, not touching six files.

## 2. What exists today

- Tools are a hard-coded list in `registry::specs()` (`crates/roost-core/src/agent/registry.rs`): `list_dir`, `read_file`, `read_binary`, `find_files`, `search_text`, `search_bytes`, `write_file`, `edit_file`, `run_command`, `ask_user`, `fetch_url`. Each has a `Family` (Files, Shell, Ask, Web) and a `Capability` (Read, Write, Run, Ask, Fetch).
- Per agent (per session) there is already an `EffectiveToolConfig`: a permission for each capability (Off / Ask / Auto) and an on/off switch for each tool. The R/W/X/N pills and the tools popover edit it.
- App-wide switches (Settings) can force a capability Off (`agents_can_write`, `agents_can_run`). Limits are app-wide (`Settings.tools: Limits`), scaled to the model's window.
- A tool's pieces are spread out: its spec in `registry.rs`, its run code in `tools.rs` / `change.rs` / `command.rs` / `web.rs` / `ask.rs`, special cases in `agent_loop.rs` (`ask_user`, `fetch_url`, writes, commands, the folder wait), its limits wording in `chat/tool_limits.rs`, its pills and cards in the GUI.

So per-agent on/off already exists. What is missing is per-agent **configuration** (limits, options), one **definition** per tool, and a way to **add** tools without editing the loop.

## 3. Proposed shape

### 3.1 One definition per tool

A `Tool` trait in core, implemented once per tool, registered in one list:

- identity: `name`, `label`, `family`, `capability`, `description` (the prompt text), `parameters` (JSON Schema);
- `needs_workspace`, `needs_network` (so the loop's special cases become data);
- `defaults`: default permission and the settings it understands (3.2);
- `plan(args) -> Plan`: what the loop must do first, as data: nothing, ask for approval with a preview (diff, command, URL), wait for a folder, or ask the user a question;
- `run(ctx, args) -> Result<String, ToolError>`: the work, with a context holding the workspace, limits, cancel flag and undo room.

`agent_loop.rs` stops matching on tool names; it asks the tool for its plan and acts on it. The existing special cases (`ask_user`, `fetch_url`, writes, commands, folder wait) become the first implementations, so they are migrated one at a time under their existing tests.

### 3.2 Per-agent settings

`EffectiveToolConfig` gains `settings: BTreeMap<String, serde_json::Value>`, keyed `tool.option` (for example `search_text.max_matches`). A tool declares its options (name, type, default, minimum, maximum, help). The effective value is: the agent's override, else the app-wide Settings value, else the default; always clamped to the declared range and to the model's window. Old session files load unchanged (serde default).

### 3.3 Where it is edited

- **Per agent:** the tools popover and family cards that exist now, extended so the open tool row shows its options (today read-only, with "Change in Settings").
- **App-wide:** Settings → Tools keeps the ceilings and defaults.
- **A tool list view** (the "comprehensive list"): every tool, grouped by family, with its description, status (on / ask / off for this agent), options, and the model-facing description. This is the same data as the popover, shown on one page.

### 3.4 Extensibility

Phase 1 (this work): built-in tools only, through the trait; adding one is one file plus registration plus tests. Phase 2 (later, its own spec): external tools. Candidates are MCP servers and user-defined command tools. Both must pass through the same permission model and the same `plan` gate; neither may bypass the Settings switches. **Decision needed:** which external kind, if any, you want first (3.5).

### 3.5 Decisions needed

1. **Presets.** Should a reusable "agent preset" exist (a named set of tool settings you pick when starting Work), or is per-session configuration enough for now? Recommendation: per-session only now; presets later, since the data model above makes them a saved `EffectiveToolConfig`.
2. **External tools first?** MCP servers, user command tools, or neither yet. Recommendation: neither yet; design phase 2 after phase 1 has run for a while.
3. **Merging tools.** The old note (4d) asked whether `read_file`/`read_binary`, `search_text`/`search_bytes`, `list_dir`/`find_files`, `write_file`/`edit_file` should merge. The hand run found nothing wrong with the split. Recommendation: keep the split; the trait makes a later merge cheap.
4. **Tool ids in saved chats.** Sessions store tool names. Recommendation: names stay stable ids forever; a renamed tool keeps an alias.

## 4. Order of work (each step ships on its own, tests first)

1. Introduce the `Tool` trait and port `list_dir` and `read_file` through it, with the old `specs()` built from the new list (no behaviour change).
2. Port the other file tools, then `run_command`, `ask_user`, `fetch_url`, replacing the name matches in `agent_loop.rs`.
3. Add `settings` to `EffectiveToolConfig` and the clamping rules; core tests for load of old files, clamping and precedence.
4. GUI: editable options in the tool rows, then the tool list view; layout tests at both sizes.
5. Docs: how to add a tool.

## 5. Risks

- The loop is the most sensitive code in the app (approvals, undo, the folder wait). Mitigation: port one tool at a time; keep every existing end-to-end test green at each step; mutation-check the switches that must reach the core.
- Special cases hide in `agent_loop.rs`; the `plan` data type must cover them before step 2 starts, or the refactor stalls. Step 1 exists to find the gaps early.
- Prompt text is part of each tool; changing a description can change model behaviour. Keep descriptions byte-identical when porting, and re-run the hand-run harness after.

## 6. Not in scope

Swarms and multi-agent orchestration, the NAS / `http_request` work (kept as handoff items), a marketplace, sandboxing beyond what exists.
