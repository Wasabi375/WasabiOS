local dap = require("dap")

dap.adapters.gdb = {
    type = "executable",
    command = "rust-gdb",
    args = {
        "--interpreter=dap",
        "--eval-command", "set print pretty on",
        "--eval-command", "wasabi_kernel::kernel_bsp_entry",
    },
}

dap.configurations.rust = {
    {
        name = "Attach Kernel(Dev)",
        type = "gdb",
        request = "attach",
        program = function()
            return "${workspaceFolder}/latest/x86_64-unknown-none/dev/wasabi-kernel/wasabi-kernel -o 0xff000000000"
        end,
        target = 'localhost:1234',
        cwd = "${workspaceFolder}",
        stopOnEntry = true,
    },
    {
        name = "Attach Tests(Dev)",
        type = "gdb",
        request = "attach",
        program = function()
            return "${workspaceFolder}/latest/x86_64-unknown-none/dev/wasabi-test/wasabi-test -o 0xff000000000"
        end,
        target = 'localhost:1234',
        cwd = "${workspaceFolder}",
        stopOnEntry = true,
    },
    {
        name = "Attach Kernel(Release)",
        type = "gdb",
        request = "attach",
        program = function()
            return "${workspaceFolder}/latest/x86_64-unknown-none/release/wasabi-kernel/wasabi-kernel -o 0xff000000000"
        end,
        target = 'localhost:1234',
        cwd = "${workspaceFolder}",
        stopOnEntry = true,
    },
    {
        name = "Attach Tests(Release)",
        type = "gdb",
        request = "attach",
        program = function()
            return "${workspaceFolder}/latest/x86_64-unknown-none/release/wasabi-test/wasabi-test -o 0xff000000000"
        end,
        target = 'localhost:1234',
        cwd = "${workspaceFolder}",
        stopOnEntry = true,
    },
    {
        name = "Fuse mount",
        type = "gdb",
        request = "launch",
        program = "${workspaceFolder}/target/debug/fuse",
        cwd = "${workspaceFolder}/wasabi-fs/fuse",
        args = "mount test_mount/ test.wfs",
        stopAtBeginningOfMainSubprogram = true,
        env = {
            RUST_LOG = "trace",
        },
    }
}
