---
title: Working with projects
description: Create, open, and share Onda programs with their source files and buffer assets.
permalink: /docs/projects/
section: tooling
eyebrow: Tooling
---

# Working with projects

Use an Onda project to keep a program's source files and buffer assets together. A project file
(`.ondaproject`) names the source file to compile and any samples, wavetables, or other data the
program uses. It can also save compile-time settings.

A single `.onda` file is enough for a simple patch. Projects are useful when a patch spans several
files, needs external data, or should be shared with its assets included.

## Create a project

Choose a new or empty directory:

```bash
onda project my-project
```

Onda creates:

```text
my-project/
├── my-project.ondaproject
├── assets/
└── code/
    └── main.onda
```

The generated project file contains:

```json
{
  "entry": "code/main.onda"
}
```

`entry` is the source file Onda starts from. Paths in the project file are relative to the directory
containing it. You can choose another layout or project filename; `code/` and `assets/` are the
default folders created by Onda.


Edit `code/main.onda`, then run the project like any other Onda file:

```bash
onda run my-project/my-project.ondaproject
```

## Package an existing program

Use `--from` to turn an existing program into a portable project. Add `--buffer` for each sample or
other buffer asset you want to include:

```bash
onda project portable-sampler \
  --from src/sampler.onda \
  --buffer sample=recording.wav \
  --buffer sequence=sequence.ondabuffer
```

The destination must be new or empty. Onda checks the program, copies its source files, updates
their imports and includes, and packages the supplied assets. The resulting project can be moved
without depending on the original files. Standard-library modules are provided by Onda and are not
copied into the project.

Each buffer name must be declared by the program, and its data must match the declared element
type and channel count. For a buffer array, select individual slots with quoted names such as
`--buffer 'piano[39]=middle-c.wav'`.

## Open and save projects

### In Onda Run

Open a `.ondaproject` file with `onda run`, the file picker, or a file drop. Onda watches the project
file, its source dependencies, and its buffer files; saving changes reloads the project.

Command-line overrides stay active while that program reloads. Unloading or opening a file again
clears them. Save constants and buffer bindings in the project file to reuse them each time you open it.

To package the program you are working on, choose **Save as project**. It saves the source files
and currently bound buffers into a new or empty directory. Existing inline assets are included,
along with any replacement files you selected in the window.

### In the browser playground

The [playground]({{ '/playground/' | relative_url }}) opens projects as ZIP files. Include the
`.ondaproject` file and all the source files and assets it needs. For example, from a repository
checkout, package the Embedded Room example with:

```bash
cd examples/projects/embedded_room
zip -r embedded-room.zip embedded-room.ondaproject code assets
```

Choose **Open project** and select the ZIP. If it contains several projects, the playground asks
which project file to open. A project file alone cannot be opened in the browser because it does
not include the sources and assets it references. Single `.onda` and `.on` files can also be opened
directly.

Choose **Download project** to save the current sources and bound buffers as a portable ZIP. You
can reopen it in the playground or extract it and run its `.ondaproject` file with the native CLI.

For complete patches with included assets, see the
[self-contained project examples]({{ '/docs/examples/' | relative_url }}#self-contained-projects).

## Project file reference

A project file is a JSON object with three fields:

| Field | Purpose |
| --- | --- |
| `entry` | Required. The source file to compile, relative to the project file. |
| `buffers` | Optional. Files or inline data bound to the program's declared buffers. |
| `constants` | Optional. Project defaults for compile-time configuration constants. |

The [JSON Schema](https://github.com/onda-lang/onda/blob/main/schemas/project.json) can be associated
with `*.ondaproject` files in your editor. Onda also checks that the named buffers and constants
match the program when compiling the project.

### File paths

Use `/` between path components and keep the entry and buffer files inside the project directory.
Absolute paths, `.` and `..` components, and symbolic links are not supported for these paths.
Source dependencies and buffer files must also be accessed without symbolic links.

For portable filenames, avoid reserved Windows names and characters, trailing dots or spaces, and
names that differ only in letter case.

While editing on a native filesystem, source imports and includes may refer to files outside the
project directory. Before sharing such a project, package it with `onda project --from` or **Save
as project** so those dependencies are included.

### Buffer bindings

Use `buffers` to bind declared buffer names to files or inline data:

```json
{
  "entry": "code/main.onda",
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
    "piano": [
      { "file": "assets/a0.ondabuffer" },
      null,
      { "file": "assets/b0.ondabuffer" }
    ]
  }
}
```

For arrays, use the declared array name, such as `piano`. Entries bind slots in order, and `null`
leaves a slot unbound. Indexed names such as `piano[0]` are used for command-line overrides only.

Inline data specifies an element type (`bool`, `i32`, `i64`, `f32`, or `f64`), a channel count, a
sample rate, and values. Values are ordered by frame, with each frame's channels next to one
another. The number of values must be divisible by the channel count. Write `i64` values as decimal
strings, such as `"9223372036854775807"`, to preserve their precision in JSON tools.

### Buffer files and overrides

Projects accept WAV files and `.ondabuffer` files. WAV files are decoded as `f32` data.
`.ondabuffer` preserves any supported element type, including integer and boolean data.

Packaging or downloading a project converts its bound buffers to `.ondabuffer` files. Onda keeps
the original asset names where possible, changing the extension to `.ondabuffer`.

Use `--buffer` with command-line playback to replace a project binding for one run:

```bash
onda run play portable-sampler/portable-sampler.ondaproject \
  --buffer sample=another-recording.wav --forever
```

This leaves the saved project unchanged. For an array slot, use a quoted argument such as
`--buffer 'piano[1]=replacement.ondabuffer'`.

### Compile-time settings

Use `constants` to save defaults for `config const` declarations in the entry program:

```json
{
  "entry": "code/main.onda",
  "constants": {
    "Enabled": true,
    "Channels": 8,
    "Seed": "9007199254740993",
    "Window": [0.0, 0.5, 1.0]
  }
}
```

Names and types must match configurable constants declared in the program. Values can be JSON
booleans, numbers, or arrays; use decimal strings for `i64` values.

A command-line selection overrides the saved value for that invocation:

```bash
onda run my-project/my-project.ondaproject --const Channels=2
```

Applications embedding Onda can also load and save projects. See the
[C API]({{ '/docs/api/' | relative_url }}#project-images-and-buffer-assets) and
[Web API]({{ '/docs/web-api/' | relative_url }}#onda-langwasm-compiler) for integration details.
