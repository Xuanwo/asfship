# Template and Artifact Reference

This document summarizes the inputs used by asfship to render GitHub Discussions and the outputs generated during prerelease and release operations.

## Discussion Templates
Templates live in `templates/` and are rendered with [Tera](https://tera.netlify.app/). Each template receives a context map derived from the current release plan.

### Common Variables
- `{repo}`: Repository name inferred from the `origin` remote.
- `{version}`: Release version without the rc suffix.
- `{rc_suffix}`: Either empty (stable) or `-rcN` for release candidates.
- `{tag}`: Fully qualified git tag (`vX.Y.Z` or `vX.Y.Z-rc.N`).
- `{main_crate}`: Name of the crate that defines the project tag series.
- `{release_date}`: ISO-8601 date generated at runtime.
- `{changelog}`: Plain-text summary assembled from per-crate changelog entries.
- `{crates}`: List containing `name`, `old_version`, `new_version`, and a formatted changelog snippet for each changed crate.
- `{artifacts}`: List of artifact metadata (`name`, `size`, `sha512`, `url`) used when assets are available.
- `{svn_url}`: Destination URL under `https://dist.apache.org/repos/dist/dev` for release candidate assets.
- `{vote_close_date}`: Optional proposed vote closing date.

### Template Roles
- `templates/start.md`: Introduces the release process and highlights planned changes.
- `templates/vote.md`: Outlines verification steps for voters and enumerates artifact checksums.
- `templates/release.md`: Announces the final release with per-crate version deltas and summary prose.

Adjust the Markdown files to customize tone or structure. Keep output in plain text or Markdown suitable for GitHub Discussions—no alternative report formats are required.

## Generated Artifacts
`asfship prerelease` packages source archives for each changed crate:
- Tarball: `apache-<repo>[-<crate>]-<X.Y.Z>[-rcN]-src.tar.gz`
- Zip: `apache-<repo>[-<crate>]-<X.Y.Z>[-rcN]-src.zip`
- Checksum: `<artifact-name>.sha512`

Artifacts land under `target/asfship/<tag>/` by default or the directory specified via `--artifact-dir`. When `--local-assets` is omitted, asfship uploads the files to the matching GitHub Release.

Use `asfship sync` to replicate the latest rc artifacts from GitHub into the ASF `dist/dev` SVN tree. Signed `.asc` files are not generated automatically; upload them manually before running `sync` so they propagate with the rest of the assets.
