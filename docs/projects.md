---
title: Onda projects
description: Editable project manifests, portable typed buffers, and immutable project images.
permalink: /docs/projects/
section: reference
eyebrow: Project format
---

# Onda projects

An Onda project is an `.ondaproject` JSON file referencing an entry source and optional buffer
assets. Its containing directory is the base and containment boundary for paths stored in the
manifest (`entry` and file-backed buffer bindings), but the project does not need a dedicated
directory. It is an editable, host-neutral representation: the CLI, Onda Run, and other hosts
resolve the same entry and logical buffer bindings.
Any basename is valid. Filesystem exports use the target name supplied by the user.

Create one with:

```bash
onda project my-project
onda compile my-project/my-project.ondaproject
onda run my-project/my-project.ondaproject
```

For complete musical uses of the format, see the checked-in
[project showcases](https://github.com/onda-lang/onda/tree/main/examples/projects): a morphable
wavetable bank, a typed-buffer score driving a modal instrument, and a stereo impulse response
driving a room effect. Each is self-contained and renders without additional host bindings.

The destination passed to `onda project` must be new or an empty directory. Onda writes the complete
project to a sibling staging directory and publishes it with one rename, so a failed export does not
leave a partially written project at the destination.

The generated layout is:

```text
my-project/
├── my-project.ondaproject
├── assets/
└── code/
    └── main.onda
```

## Project file

The generated `my-project.ondaproject` starts as:

```json
{
  "entry": "code/main.onda"
}
```

`entry` and buffer file paths are Unicode NFC-normalized, project-relative UTF-8 paths using `/`.
Each path component is at most 255 UTF-8 bytes. Absolute paths, `.`, `..`, empty components,
backslashes, control characters, Windows-reserved characters and device names, and components
ending in a dot or space are rejected. Referenced files must also remain distinct under portable
Unicode case folding and cannot conflict as both a file and an ancestor directory. Paths which
traverse symlinks are rejected. This keeps editable filesystem projects and live watching bound to
stable paths; immutable captured project images are unaffected.

The manifest's containing directory is its project root. A manifest may occur at any path in a
larger file set; its `entry` and file-backed buffers are resolved relative to that directory. The
format assigns no meaning to directory names such as `code`, `src`, `assets`, or `media`.

That manifest containment does not restrict normal include/import resolution while editing on a
native filesystem. Source references may reach outside the manifest's directory, so an editable
workspace is not necessarily self-contained. `onda project --from` and **Save as project** capture
the exact reachable graph, relocate its entry to `code/main.onda`, preserve meaningful source
subdirectories below `code/`, and rewrite references syntax-aware. Sources outside the capture root
are placed below `code/external/`. Those packaged exports are self-contained and portable to
filesystem-free hosts.

## Packaging an existing source

`--from` captures the exact reachable source graph, relocates files outside the entry directory,
and rewrites non-standard-library imports and includes syntax-aware:

```bash
onda project portable-sampler \
  --from src/sampler.onda \
  --buffer sample=recording.wav \
  --buffer sequence=sequence.ondabuffer
```

The exported `code/`, `assets/`, and `code/main.onda` layout is a publication convention, not a
required project shape. An `.ondaproject` file may instead live alongside existing repository
sources and assets, name any contained entry, and bind assets at any contained relative path. Each `--buffer` name must
be declared by the compiled program. Its element type and static channel
count must match the declaration. WAV inputs become canonical `f32` assets; `.ondabuffer` preserves
any supported primitive type. The resulting directory no longer depends on the original source or
buffer paths.

For a fixed buffer array, CLI bindings address physical slots by name, such as
`--buffer 'piano[39]=middle-c.wav'`. Unmentioned scalar buffers and array slots remain neutral and
are not written into the project. A project therefore records available assets, not a requirement
that every declared resource be populated.

`buffers` maps each declared Onda buffer name to either a file, typed inline data, or an array of
those bindings. Array entries bind slots in declaration order; `null` leaves a slot neutral and
unbound. Unlike CLI overrides, the manifest uses the logical array name (`piano`), not flat keys
such as `piano[0]`:

```json
{
  "buffers": {
    "impulse": {
      "file": "assets/impulse.wav"
    },
    "steps": {
      "inline": {
        "element": "i32",
        "channels": 1,
        "sample_rate": 48000,
        "values": [0, 4, 7, 12]
      }
    },
    "large_ids": {
      "inline": {
        "element": "i64",
        "channels": 1,
        "sample_rate": 1,
        "values": ["0", "9223372036854775807"]
      }
    },
    "piano": [
      { "file": "assets/a0.ondabuffer" },
      null,
      { "file": "assets/b0.ondabuffer" }
    ]
  }
}
```

Inline values are frame-major and interleaved by channel. Their element type must exactly match
the declaration in Onda source. `i64` values are decimal strings so JSON tooling cannot round
them through an inexact number representation.

The optional checked-in [JSON Schema](../schemas/project.json) can be associated with
`*.ondaproject` by editor tooling, but project files do not embed a schema URL. Onda performs
stricter validation when loading them, including channel divisibility, numeric conversion,
resource limits, portable filenames, and filesystem containment.

## Buffer files

Projects accept:

- `.ondabuffer`, Onda's canonical lossless typed buffer container for `bool`, `i32`, `i64`, `f32`,
  and `f64`.
- WAV input as a convenience adapter. WAV data is decoded to an `f32` buffer.

An `.ondabuffer` stores the element type, frames, channels, sample rate, frame-major interleaved
payload, and a SHA-256 content digest. Integer and floating-point payloads use fixed-width
little-endian encoding; booleans are exactly `0` or `1`. Project images and materialized exports
use `.ondabuffer`, so portability does not depend on a host's audio codecs.

Exports preserve the original asset basename when the host knows it, replacing the extension with
`.ondabuffer`. Assets without filename provenance use their logical buffer name. Content hashes remain
the immutable asset identity and are added to filenames only when needed to resolve a portable
filename collision.

`--buffer name=path` remains available for `onda run play` and `onda run render`. It overrides a
project binding with the same physical name for that invocation. A fixed-array slot uses a quoted
shell argument such as `--buffer 'piano[1]=replacement.ondabuffer'`.

## GUI workflows

The egui and webview run hosts expose one **Open Onda source or project** importer accepting `.onda`
and `.ondaproject` files. Dropping either input onto the window works as well.

While an editable project is open, the native run host watches the selected manifest and its
file-backed buffer assets in addition to the entry and transitive non-standard-library sources.
Changing any of them reloads the project; inline assets change when the manifest changes.
Filesystem-backed Onda inputs, source dependencies, and project assets must not traverse symlinks;
the loader reports the offending component instead of establishing ambiguous live-watch semantics.

Once a source or project is loaded, **Save as project** captures the exact reachable sources and the
currently bound buffers into a new portable project directory. Existing inline project assets are
preserved, and file bindings selected in the host replace the corresponding packaged assets. The
destination must be new or empty so publication remains atomic.

The browser playground provides **Open project** and **Download project** controls. It opens a
single `.onda` file or a project ZIP containing one or more `.ondaproject` files. When an
archive contains several projects, the playground asks which manifest to open. Downloading creates
a ZIP from the current in-memory source files and bound buffers. Buffer payloads are canonicalized
as `.ondabuffer`, including `bool`, `i32`, `i64`, `f32`, and `f64` data. The ZIP is only a browser
transport: after extraction, open any `.ondaproject` file with the native CLI or run hosts.

## Immutable project images

The `onda_project` crate also defines `ProjectImage`, an immutable checkpoint intended for DAW
state, browser tooling, and other hosts. One image contains:

- the relocated entry identity;
- the exact built-in standard-library fingerprint;
- exact source documents;
- resolved include/import edges;
- logical buffer-name to content-addressed asset bindings;
- canonical typed assets;
- a schema version and root content digest.

`SourceImage::capture` converts a successful frontend source manifest into a portable graph and
rewrites include/import references syntax-aware. `SourceImage::replay` loads that graph without
consulting the filesystem and rejects a mismatched built-in standard library.
Loading an editable project treats its entry and every unclaimed `.onda` or `.on` file as a UTF-8
source document; project manifests and file-backed buffer bindings take precedence over filename
extensions. This preserves extensionless entry files and work-in-progress sources which are not
reachable from the entry. The reachable graph must load and parse successfully; unreachable
documents are preserved verbatim and do not participate in compilation.
When a file set contains multiple manifests, they form one shared source workspace. Every valid
manifest participates in classifying manifest and buffer-asset paths, while the selected
manifest alone chooses the entry and active buffer bindings. This lets several projects share and
cross-reference source files without mistaking another project's `.onda`-named asset for source.
`ProjectImage::serialize` produces the bounded,
versioned binary image; `ProjectImage::deserialize` verifies every asset and the root digest before
publication. `materialization_plan` returns relative filenames and bytes without writing the
filesystem, leaving atomic publication policy to the host.

The native C runtime treats assets from editable filesystem projects and immutable images as program
defaults. `onda_compile_file` performs source analysis and code generation before decoding external
assets, then makes those decoded assets part of the compiled program without constructing a portable
project image. `onda_project_image_compile` instead retains shared ownership of the image's decoded
assets. Every instance initially binds the program-owned sample storage without copying it. A project
binding is rejected when reachable Onda code may write that physical buffer slot. Hosts can replace a
default with `onda_bind_buffer`, unbind it to obtain the neutral buffer behavior, or restore it with
`onda_reset_buffer_to_project_default`. Instances retain their compiled program and project assets,
so destroying the original program or image handle does not invalidate their bindings.

## Native and web API parity

The C API and `@onda-lang/wasm-compiler` expose the same project operations over the same
`onda_project` implementation. The terminology separates an ephemeral source workspace from a
portable project image:

| Operation | C API | Web compiler |
| --- | --- | --- |
| Compile an editable filesystem source or project | `onda_compile_file` | — |
| Compile exact in-memory sources | `onda_compile_source_graph` | `compileWorkspace` |
| Capture/build an image | `onda_project_image_capture` | `createProjectImage` |
| Load a materialized project file set | `onda_project_image_load_files` | `loadProjectFiles` |
| Serialize or inspect an image | `onda_project_image_serialize`, `onda_project_image_*` getters | `createProjectImage`, `inspectProjectImage` |
| Compile an image | `onda_project_image_compile` | `compileProjectImage` |
| Produce relative files and bytes | `onda_project_image_materialize` | `materializeProjectImage` |
| Encode/decode typed buffers | `onda_buffer_asset_encode`, `onda_buffer_asset_decode` | `encodeBufferAsset`, `decodeBufferAsset` |
| Query immutable format contracts | `onda_project_image_format_version`, `onda_buffer_asset_format_version`, `onda_current_stdlib_digest` | `projectCapabilities` |

`onda_compile_file` is the native editable-filesystem entry point: it accepts `.onda`, `.on`, and
`.ondaproject`, attaches project buffers as immutable defaults, and returns a source manifest whose
deduplicated watch projection includes the selected input, resolved and unresolved source graph,
project manifest, declared entry, and file-backed assets. Missing dependency, entry, and asset paths
remain in the projection on failure so their creation can recover the project. The host owns the
polling or OS-watcher mechanism and recompiles the same input after a relevant change.

The web methods return JavaScript objects and typed arrays; C uses opaque handles and two-pass
buffer sizing. Those are transport differences only. Image serialization, content and asset
digests, path validation, source replay, resource limits, and materialized files are canonical Rust
operations shared by both. `loadProjectFiles(files, projectFilePath)` and
`onda_project_image_load_files(..., project_file_path_utf8, ...)` accept an explicit manifest
path when the file set contains multiple projects; omitting it requires an unambiguous manifest.
Portable project exports require a successful compilation. Loading extracted project files rejects
a reachable source graph that cannot be loaded and parsed, while
compilation also verifies that each bound asset names a declared buffer and matches its primitive
element type and fixed channel count.
