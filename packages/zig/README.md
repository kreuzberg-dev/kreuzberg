# xberg

High-performance document intelligence library

## Installation

Install Zig from [ziglang.org](https://ziglang.org/download/).

## Building

```sh
zig build
zig build test
```

## Usage

Add to your `build.zig.zon`:

```text
.dependencies = .{
    .xberg = .{
        .path = "path/to/xberg",
    },
},
```

## License

MIT
