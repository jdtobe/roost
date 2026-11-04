# Roadmap

This roadmap is drawn from the design spec (`docs/superpowers/specs/2026-10-22-hf2ollama-design.md` (the importer, written before the rename to Roost) and the Roost workbench design `docs/superpowers/specs/2026-10-27-roost-workbench-design.md`): its scope, acceptance criteria, known risks and non-goals. It is a statement of intent, not a schedule. There are no release dates and no releases yet.

## Roost workbench stages

The product is being extended from an importer into a local-model workbench (chat, agents, incognito sessions, branches and history). Stages 1 to 3 and most of 4 exist; automatic titles and the history manager are planned, not shipped.

1. **Foundation (implemented):** the rename to Roost, safe migration of hf2ollama settings, import history, staging markers and (on request) the Hugging Face token, four theme families with light and dark variants, and the serializable session domain types in `roost-core`.
2. **Streaming chat and history (implemented):** streaming chat, one file per session with a rebuildable index, incognito, the history rail with pinning, and background runs. Deferred inside this stage: markdown and code-block rendering, a copy button, regenerate, rail search and filters, and pagination of the Earlier group.
3. **Agents (implemented):** the Work tab, a workspace jail, the tool registry (Files: list, find, search, read, read bytes, write, edit; Shell: run a command, opt-in), steering, Stop and a step budget. The agent can ask the user a question (`ask_user`: pick one, pick many or type; up to 5 per run). Per tool kind Off, Ask or Allow, with approval cards (diffs for writes, the exact command for runs), undo for writes, configurable tool limits, and notifications while the window is in the background. Each tool is one `Tool` implementation (see `docs/ADDING_A_TOOL.md`). Not yet: network tools beyond fetching a page, MCP tools.
4. `/btw` side questions and branches are implemented. Automatic titles and the history manager are not started.

## Done

- Core library: Hugging Face and Ollama clients, compatibility classification, fit estimation, hardware detection, settings and portable paths, staging and cleanup, history, import pipeline, job manager, model card fetch, token check.
- Desktop app: window shell, Browse (search, filters, details, model card, import plan), downloads dock, Installed, Settings, light and dark themes, startup sweep of orphaned job folders.
- Retry of the download step (3 times, backoff) after network errors, 5xx and 429; a confirmation when closing the window with running downloads; a name-collision re-check before `create` and protection against starting the same name twice; settings enum values written in kebab-case (old values still load); Clear cached files also removes failed downloads.
- Downloads dock: a rolling five-second speed beside the download percentage and a five-line, scrollable log drawer above the dock.
- macOS release packaging: a universal `.app` with an icon inside a drag-install `.dmg`, ad-hoc signed and verified by a packaging script.
- Project files, CI (fmt, clippy, tests on three OSes, `dev-hooks` job) and a manual release workflow (automatic releases on merge to `main` are the planned next step; see the comment in `release.yml`) and cargo-deny.
- A real end-to-end run against a throwaway Ollama 0.40.8 on macOS; see [the acceptance record](superpowers/notes/acceptance.md).

See [CHANGELOG.md](../CHANGELOG.md).

## Now

- Finish the visual verification that needs an unlocked display and capture the README screenshots (listed in the acceptance record).
- Run the CI matrix for the first time on Linux and Windows and fix whatever it finds; verify the Linux package list.
- Release packaging: macOS is a universal `.app` inside a drag-install `.dmg` (icon, ad-hoc signature and CI verification included); Windows and Linux still ship as bare-binary archives with combined third-party license notices.

## Next

Known limitations to address after the first release candidate:

- Resume of failed downloads: a Retry or Resume action for a failed job that reuses the kept partial files, including after a restart (today the files are kept for 7 days but a new import starts over).
- An overwrite prompt for a name that already exists (today the import is refused and overwrite is not offered from the GUI).
- Ollama does not report free space or the model store path, so the Installed usage bar shows two segments (models imported with this app, and all others) and the status bar omits the store path.
- Each settings change creates a new job manager and keeps the old ones, so a long session leaks a few idle managers and the parallel limit is per manager. Prune idle managers.
- Pin the CI actions to full commit SHAs.
- The folder dialog (`rfd`) blocks the UI thread while open and returns nothing if no XDG portal is running on Linux.
- Screenshots in the README.
- Verify on Linux and Windows: Ollama safetensors `quantize` values (verified on macOS and Ollama 0.40.8 only; `int8` and no quantization were imported end to end, `int4` was not) and general rendering.
- Verify the supported-architecture list with more real test imports; only `LlamaForCausalLM` has been imported. The list is data and can be updated as Ollama's support changes.
- Verify `quantize` on a GGUF create and the error behavior for an unsupported architecture.
- Warn in the UI when Ollama reports a version below the minimum verified version (0.40.8).

## Later

- **Chat and Work redesign**, carrying over what the Models tab settled on: the theme's own colours in stacked bands, icon buttons instead of text buttons, pills for facts, and a more modern conversation layout.
- **Agent sessions without a folder.** Done (PR 63): New Work opens a session with no folder and no model; a file or command tool pauses the run until a folder is chosen or skipped, and the model popup marks which models support tools.
- A less mechanical voice for Read aloud (the system tools use the default synthetic voices).
- VRAM detection beyond NVIDIA and Apple Silicon (for example AMD and Intel), subject to reliable detection.
- Accessibility improvements; egui has weaker accessibility than native toolkits.
- A command-line front end on top of `Roost-core` (the library split leaves room for it; not planned in detail).
- Handling of orphan blobs if Ollama ever adds a blob-delete API (today they are documented, not worked around).

## Non-goals for v1

These are deliberately out of scope:

- Running or chatting with models in v1 of the importer. Chat is planned in the Roost workbench stages above.
- Editing Modelfiles beyond the model name.
- Converting architectures Ollama does not support (no Python or llama.cpp conversion step).
- Uploading models to Hugging Face.
- Managing Ollama's own library pulls.
