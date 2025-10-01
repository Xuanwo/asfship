# Advanced Configuration & Customization

This guide covers the knobs that tailor asfship to match your workspace layout and release processes. Most projects can run with zero setup; only introduce these configurations when the automatic inference needs help.

## Global CLI Flags
- `--dry-run`: Skip git mutations, network calls, and filesystem writes that would change state. Most commands print the planned actions so you can review them beforehand.
- `--artifact-dir <path>`: Override the directory used for packaging release artifacts. Defaults to `target/asfship/<tag>` when omitted.
- `--local-assets`: Keep packaged artifacts on disk without pushing tags or uploading to GitHub Releases. Combine with `--artifact-dir` for full control over output locations.

## Configuration File (`.asfship.toml`)
Place a minimal TOML file at the repository root only when the automatic main-crate inference is ambiguous.

```toml
# .asfship.toml
main_crate = "your-main-crate-name"
```

The resolver searches for `.asfship.toml` in the workspace root. No other configuration keys are currently supported; keep the file focused on disambiguation.

## Environment Variables
- `ASFSHIP_GITHUB_TOKEN`: GitHub personal access token used for Discussions, Releases, and asset uploads. The token must grant `repo` scope for private repositories. Commands that require GitHub write access abort when this variable is missing or empty. When present, asfship builds an authenticated `octocrab` client; otherwise some flows fall back to invoking the `gh` CLI if installed.

## External Tools
- `svn`: Required for `asfship sync` to push release candidate artifacts into the ASF `dist/dev` tree. Ensure the command is available on `PATH` and that your environment has valid ASF SVN credentials.
- `gh`: Optional but recommended. When the GitHub token is absent or certain API operations need CLI fallback, asfship shells out to `gh`.

## Template Overrides
Built-in templates live under `templates/`. You can adjust wording or structure by editing those Markdown files directly. Each command loads the template at runtime, so repo-local modifications take effect immediately without recompilation.

## Workspace Expectations
- The workspace must adhere to Conventional Commits so the prerelease planner can derive SemVer bumps.
- Tags follow the pattern `vX.Y.Z` for stable releases and `vX.Y.Z-rc.N` for release candidates. Ensure previous releases use the same pattern so auto-increment works.

If additional customization hooks become necessary (for example, alternative artifact naming or non-ASF distribution targets), track them in the project backlog before extending the CLI surface.
