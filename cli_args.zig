const std = @import("std");
const testing = std.testing;

/// A struct to represent a CLI argument along with its i64 representation
const FlagOrCmd = struct {
    /// The arg converted to an i64 value
    as_i64: i64,
    /// An optional pointer to the next arg (relevant with `=`, e.g. `roc --foo=bar`)
    equals_val: ?[*:0]const u8,

    return FlagOrCmd{
        .as_i64 = if (is_flag) -answer else answer,
        .equals_val = null,
    };
}
        // Both of these scenarios should be extremely rare, so doing a branch for them.
        if (arg[0] == 0 or arg[1] == 0) {
            return FlagOrCmd{
                .as_i64 = 0,
                .equals_val = null,
            };
        }

        // Branchlessly skip over a leading "-" (and also a leading "--"),
        // while recording that this is a flag if it started with "-" or "--".
        // At this point we know length >= 2, so we can index into [0] and [1].
        const is_flag = arg[0] == '-'; // If it starts with a dash, it's a flag.
        const second_byte_is_dash = arg[1] == '-';

        // Skip the dash(es) we just saw.
        // (We treat `-foo` and `--foo` as equivalent, and intentionally don't offer
        // shorthands like `-f` being shorthand for `--foo`.)
        arg = arg[is_flag + (is_flag and second_byte_is_dash) ..];

        var answer: i64 = 0;
        var index: usize = 0;
        var char = arg[index];

        while (char != 0) : (index += 1) {
            char = arg[index];

            switch (char) {
                '=' => {
                    // Found an '=' in the argument; return what we've parsed so far,
                    // and make equals_val point to what comes after the '='
                    return FlagOrCmd{
                        .as_i64 = if (is_flag) -answer else answer,
                        .equals_val = @ptrCast(arg[index + 1 ..].ptr),
                    };
                },
                0 => {
                    // We've hit the end of the string; we're done!
                    return FlagOrCmd{
                        .as_i64 = if (is_flag) -answer else answer,
                        .equals_val = null,
                    };
                },
                else => {
                    if (index < @sizeOf(i64)) {
                        // Write the byte to the appropriate position in the i64 answer
                        answer |= @as(i64, char) << (index * @sizeOf(i64));
                    } else {
                        // `roc` subcommands and flags all have length <= 8, so
                        // if we're about to look at the 9th char, we're done.
                        return FlagOrCmd{
                            .as_i64 = 0,
                            .equals_val = null,
                        };
                    }
                },
            }
        }
    }
};

// Subcommands
const CMD_VERSION = parse_flag_or_cmd("version").as_i64;
const CMD_CHECK = parse_flag_or_cmd("check").as_i64;
const CMD_BUILD = parse_flag_or_cmd("build").as_i64;
const CMD_HELP = parse_flag_or_cmd("help").as_i64;

// Flags
const FLAG_VERSION = parse_flag_or_cmd("-version").as_i64;
const FLAG_OPTIMIZE = parse_flag_or_cmd("-optimize").as_i64;
const FLAG_HELP = parse_flag_or_cmd("-help").as_i64;
const FLAG_MAIN = parse_flag_or_cmd("-main").as_i64;
const FLAG_TIME = parse_flag_or_cmd("-time").as_i64;
const FLAG_V = parse_flag_or_cmd("-v").as_i64;
const FLAG_H = parse_flag_or_cmd("-h").as_i64;

fn process_args(args: std.process.ArgIterator, allocator: std.mem.Allocator) !void {
    // Skip the first arg (path to the executable)
    _ = args.next();

    // The next arg is the subcommand
    if (args.next()) |second_arg| {
        const info = parse_flag_or_cmd(second_arg);

        if (info.equals_val != null) {
            // We found an `=` in the subcommand.
            // This means we got something like `roc foo=bar`, which
            // we should treat as a filesystem path that has an `=` in it.
            return run(second_arg, args, allocator);
        }

        switch (info.as_i64) {
            CMD_VERSION, FLAG_V => {
                return print_version();
            },
            FLAG_VERSION => {
                if (args.next() == null) {
                    // `roc --version` with no other args just prints the version
                    return print_version();
                } else {
                    // TODO download the requested roc version and forward the other args along to it.
                }
            },
            CMD_CHECK => {
                return check(args);
            },
            CMD_BUILD => {
                return build(args);
            },
            CMD_HELP, FLAG_HELP, FLAG_H => {
                // `roc help`, `roc -help`, `roc --help`, `roc -h`, and `roc --h` all print help.
                return print_help("");
            },
            else => {
                // If there's no recognized subcommand, then we assume we were
                // given a Roc source code file to execute (e.g. `roc foo`).
                //
                // Note that the path doesn't necessarily end in .roc, because it
                // might be a script being executed via `#!/bin/env roc`
                return run(second_arg, args);
            },
        }
    } else {
        // `roc` was called with no args, so default to `roc main.roc`
        return run("main.roc", args);
    }
}

/// Optimization level for code generation
const Optimize = enum {
    /// Optimize for runtime performance
    Perf,
    /// Optimize for compiled binary size
    Size,
    /// Optimize for development speed (completing the build as fast as possible)
    Dev,
};

fn run(first_arg: []const u8, other_args: std.process.ArgIterator, allocator: std.mem.Allocator) !void {
    var arg = first_arg;
    var file_path_arg: ?[]const u8 = null;
    var optimize: Optimize = .Dev;
    var time: bool = false;

    while (true) {
        const info = parse_flag_or_cmd(arg);

        switch (info.as_i64) {
            FLAG_OPTIMIZE => {
                // TODO extract this logic and use it in `build` too.
                if (info.equals_val) |equals_val| {
                    // TODO convert to u32 and switch on that.
                    if (std.mem.eql(u8, std.mem.span(equals_val), "perf")) {
                        optimize = .Perf;
                    } else if (std.mem.eql(u8, std.mem.span(equals_val), "size")) {
                        optimize = .Size;
                    } else if (std.mem.eql(u8, std.mem.span(equals_val), "dev")) {
                        optimize = .Dev;
                    } else {
                        // TODO make this error nicer, explain what values are accepted.
                        std.debug.print("Error: invalid value for --optimize\n", .{});
                        return;
                    }
                } else {
                    // If we didn't have an `=` after --optimize, default to Perf.
                    optimize = .Perf;
                }
            },
            FLAG_TIME => {
                if (info.equals_val) |equals_val| {
                    // TODO convert to u64 and switch on that.
                    if (std.mem.eql(u8, std.mem.span(equals_val), "true")) {
                        time = true;
                    } else if (std.mem.eql(u8, std.mem.span(equals_val), "false")) {
                        time = false;
                    } else {
                        // TODO make this error nicer, explain that it's either true or false
                        std.debug.print("Error: invalid value for --time\n", .{});
                        return;
                    }
                } else {
                    // If we didn't have an `=` after --time, default to true.
                    time = true;
                }
            },
            else => {
                if (file_path_arg != null) {
                    // TODO make this error nicer.
                    std.debug.print("Error: multiple filenames given\n", .{});
                    return;
                }

                file_path_arg = arg;
            },
        }

        // Check if there are more args
        if (other_args.next()) |next_arg| {
            arg = next_arg;
        } else {
            break; // No more args to process
        }
    }

    const file_path = file_path_arg orelse {
        // TODO make this error nicer
        std.debug.print("Error: no file path provided\n", .{});
        return;
    };

    // TODO now use file_path to run.
}

fn print_help(command: []const u8) void {
    std.debug.print("Usage: roc {s} [options] [filename]\n", .{command});
    std.debug.print("Options:\n", .{});
    std.debug.print("  --help      Print this help message\n", .{});
    std.debug.print("  --optimize  Optimize the code\n", .{});
}

fn check(args: std.process.ArgIterator) void {
    var filename: ?[]const u8 = null;
    var optimize = false;
    var main: ?[]const u8 = null;

    while (args.next()) |arg| {
        const arg_u64 = parse_flag_or_cmd(arg);

        switch (arg_u64) {
            FLAG_OPTIMIZE => {
                optimize = true;
            },
            FLAG_MAIN => {
                if (args.next()) |main_arg| {
                    main = main_arg;
                } else {
                    std.debug.print("Error: --main flag requires a value\n", .{});
                    return;
                }
            },
            CMD_HELP, FLAG_HELP, FLAG_H => {
                // If we get `roc check --help`, bail out and print help info for `roc check`.
                return print_help("check");
            },
            else => {
                // Not a recognized flag, assume it's a filename
                if (filename != null) {
                    std.debug.print("Error: too many filenames given\n", .{});
                    return;
                }

                filename = arg;
            },
        }
    }

    if (filename) |file| {
        std.debug.print("Checking file: {s}\n", .{file});
    } else {
        std.debug.print("No file specified for checking\n", .{});
    }
}

fn build(args: std.process.ArgIterator) void {
    // todo: implementation goes here - accept dash-dash args before the filename
}

fn print_version() void {
    // TODO: Print version
}

test "arg_info" {
    // Test with short string
    try testing.expectEqual(@as(i64, 0x6f6c6c6568), parse_flag_or_cmd("hello").as_i64);

    // Test with 8-byte string
    try testing.expectEqual(@as(i64, 0x74736574676e6f6c), parse_flag_or_cmd("longtest").as_i64);

    // Test with longer string
    try testing.expectEqual(@as(i64, 0), parse_flag_or_cmd("this_is_longer_than_8_bytes").as_i64);

    // Test with empty string
    try testing.expectEqual(@as(i64, 0), parse_flag_or_cmd("").as_i64);
}

/// Optimization level for code generation
const Optimize = enum {
    /// Optimize for runtime performance
    Perf,
    /// Optimize for compiled binary size
    Size,
    /// Optimize for development speed
    Dev,
};

fn process_args(args: std.process.ArgIterator, allocator: std.mem.Allocator) !void {
    _ = args.next(); // Skip executable name

    const second_arg = args.next() orelse {
        return run("main.roc", args, allocator);
    };

    const info = parse_flag_or_cmd(second_arg);
    if (info.equals_val != null) {
        return run(second_arg, args, allocator);
    }

    switch (info.as_i64) {
        CMD_VERSION, FLAG_VERSION, FLAG_V => return print_version(),
        CMD_CHECK => return check(args, allocator),
        CMD_BUILD => return build(args, allocator),
        CMD_HELP, FLAG_HELP, FLAG_H => return print_help(""),
        else => return run(second_arg, args, allocator),
    }
}

fn run(first_arg: []const u8, args: std.process.ArgIterator, allocator: std.mem.Allocator) !void {
    _ = allocator;
    var optimize: Optimize = .Dev;
    var time = false;

    var it = args;
    while (it.next()) |arg| {
        const info = parse_flag_or_cmd(arg);
        switch (info.as_i64) {
            FLAG_OPTIMIZE => {
                if (info.equals_val) |val| {
                    const val_str = std.mem.span(val);
                    if (std.mem.eql(u8, val_str, "perf")) {
                        optimize = .Perf;
                    } else if (std.mem.eql(u8, val_str, "size")) {
                        optimize = .Size;
                    } else if (std.mem.eql(u8, val_str, "dev")) {
                        optimize = .Dev;
                    } else {
                        std.debug.print("Error: invalid value for --optimize\n", .{});
                        return;
                    }
                } else optimize = .Perf;
            },
            FLAG_TIME => time = info.equals_val == null or std.mem.eql(u8, std.mem.span(info.equals_val.?), "true"),
            else => {},
        }
    }

    _ = first_arg;
    _ = optimize;
    _ = time;
}

fn check(args: std.process.ArgIterator, allocator: std.mem.Allocator) !void {
    _ = allocator;
    var it = args;
    while (it.next()) |arg| {
        const info = parse_flag_or_cmd(arg);
        switch (info.as_i64) {
            FLAG_HELP, FLAG_H => return print_help("check"),
            else => {},
        }
    }
}

fn build(args: std.process.ArgIterator, allocator: std.mem.Allocator) !void {
    _ = args;
    _ = allocator;
}

fn print_version() void {
    std.debug.print("Roc version 0.1.0\n", .{});
}

fn print_help(command: []const u8) void {
    if (command.len > 0) {
        std.debug.print("Usage: roc {s} [options]\n", .{command});
    } else {
        std.debug.print("Usage: roc [command] [options]\n", .{});
    }
    std.debug.print("\nCommands:\n", .{});
    std.debug.print("  build     Build a Roc project\n", .{});
    std.debug.print("  check     Check a Roc project\n", .{});
    std.debug.print("  help      Show this help message\n", .{});
    std.debug.print("  version   Show version information\n", .{});
    std.debug.print("\nOptions:\n", .{});
    std.debug.print("  --help, -h     Show this help message\n", .{});
    std.debug.print("  --version, -v  Show version information\n", .{});
}

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    const allocator = gpa.allocator();
    defer _ = gpa.deinit();
    
    try process_args(std.process.args(), allocator);
}
