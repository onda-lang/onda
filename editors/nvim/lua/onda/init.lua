local M = {}

local defaults = {
  server_path = "onda",
  server_args = {},
  preview_path = nil,
  preview_args = {},
  preview_host = nil,
  root_markers = { "Cargo.toml", ".git" },
}

local state = {
  opts = vim.deepcopy(defaults),
  initialized = false,
}

local function is_windows()
  return vim.fn.has("win32") == 1 or vim.fn.has("win64") == 1
end

local function path_exists(path)
  local stat = (vim.uv or vim.loop).fs_stat(path)
  return stat ~= nil
end

local function join_path(...)
  local sep = is_windows() and "\\" or "/"
  return table.concat({ ... }, sep)
end

local function parent_dir(path)
  local parent = vim.fn.fnamemodify(path, ":h")
  if parent == path then
    return nil
  end
  return parent
end

local function find_root(path)
  local dir
  if path == "" then
    dir = vim.fn.getcwd()
  elseif path_exists(path) then
    dir = vim.fn.fnamemodify(path, ":p:h")
  else
    dir = vim.fn.fnamemodify(path, ":p")
  end

  while dir and dir ~= "" do
    for _, marker in ipairs(state.opts.root_markers) do
      if path_exists(join_path(dir, marker)) then
        return dir
      end
    end
    dir = parent_dir(dir)
  end

  return vim.fn.getcwd()
end

local function onda_lsp_cmd()
  local cmd = { state.opts.server_path }
  vim.list_extend(cmd, state.opts.server_args)
  table.insert(cmd, "lsp")
  return cmd
end

local function onda_preview_cmd(path)
  local cmd = { state.opts.preview_path or state.opts.server_path, "preview", path }
  if state.opts.preview_host == "webview" then
    table.insert(cmd, "--webview")
  end
  vim.list_extend(cmd, state.opts.preview_args)
  return cmd
end

local function notify(msg, level)
  vim.notify(msg, level or vim.log.levels.INFO, { title = "Onda" })
end

local function apply_highlights()
  local set_hl = vim.api.nvim_set_hl
  set_hl(0, "@lsp.type.port.onda", { default = true, link = "@lsp.type.parameter" })
  set_hl(0, "@lsp.type.state.onda", { default = true, link = "@variable.parameter" })
  set_hl(0, "@lsp.type.enumMember.onda", { default = true, link = "Constant" })
  set_hl(0, "@lsp.type.function.onda", { default = true, link = "Function" })
  set_hl(0, "@lsp.type.type.onda", { default = true, link = "Type" })
  set_hl(0, "@lsp.type.namespace.onda", { default = true, link = "Include" })
end

local function powershell_quote(value)
  return "'" .. value:gsub("'", "''") .. "'"
end

local function spawn_detached_windows(cmd, cwd)
  local powershell = vim.fn.exepath("powershell.exe")
  if powershell ~= "" then
    local script_path = vim.fn.tempname() .. ".ps1"
    local script_lines = {
      "$filePath = " .. powershell_quote(cmd[1]),
      "$workingDirectory = " .. powershell_quote(cwd),
    }
    local arg_expr = {}
    for _, arg in ipairs(vim.list_slice(cmd, 2)) do
      table.insert(arg_expr, powershell_quote(arg))
    end
    table.insert(script_lines, "$arguments = @(" .. table.concat(arg_expr, ", ") .. ")")
    table.insert(
      script_lines,
      "Start-Process -FilePath $filePath -WorkingDirectory $workingDirectory -WindowStyle Hidden -ArgumentList $arguments | Out-Null"
    )
    vim.fn.writefile(script_lines, script_path)

    local shell_cmd = table.concat({
      vim.fn.shellescape(powershell),
      "-NoProfile",
      "-NonInteractive",
      "-WindowStyle Hidden",
      "-ExecutionPolicy Bypass",
      "-File",
      vim.fn.shellescape(script_path),
    }, " ")
    local output = vim.fn.system(shell_cmd)
    local code = vim.v.shell_error
    vim.fn.delete(script_path)
    if code == 0 then
      return true
    end
    if output ~= "" then
      notify(
        "PowerShell preview launch failed, falling back to direct launch: " .. output,
        vim.log.levels.WARN
      )
    end
  end

  local job_id = vim.fn.jobstart(cmd, { cwd = cwd, detach = true })
  return job_id > 0
end

local function spawn_detached(cmd, cwd)
  local uv = vim.uv or vim.loop
  if is_windows() then
    return spawn_detached_windows(cmd, cwd)
  end

  local handle
  handle = uv.spawn(cmd[1], {
    args = vim.list_slice(cmd, 2),
    detached = true,
    hide = true,
    cwd = cwd,
    stdio = { nil, nil, nil },
  }, function(code)
    if handle and not handle:is_closing() then
      handle:close()
    end
    if code == 0 then
      return
    end
    vim.schedule(function()
      notify(
        ("Onda preview exited with code %d."):format(code),
        vim.log.levels.ERROR
      )
    end)
  end)

  if not handle then
    return false
  end
  uv.unref(handle)
  return true
end

function M.start_lsp(bufnr)
  bufnr = bufnr or vim.api.nvim_get_current_buf()
  if vim.bo[bufnr].filetype ~= "onda" then
    return
  end
  if #vim.lsp.get_clients({ bufnr = bufnr, name = "onda" }) > 0 then
    return
  end

  local name = vim.api.nvim_buf_get_name(bufnr)
  vim.lsp.start({
    name = "onda",
    cmd = onda_lsp_cmd(),
    root_dir = find_root(name),
    single_file_support = true,
  }, { bufnr = bufnr })
end

function M.run_patch(opts)
  opts = opts or {}
  local bufnr = opts.bufnr or vim.api.nvim_get_current_buf()
  if vim.bo[bufnr].filetype ~= "onda" then
    notify("Current buffer is not an Onda file.", vim.log.levels.ERROR)
    return
  end

  local path = vim.api.nvim_buf_get_name(bufnr)
  if path == "" then
    notify("Onda preview requires a file on disk.", vim.log.levels.ERROR)
    return
  end

  if vim.bo[bufnr].modified then
    local wrote = pcall(function()
      vim.api.nvim_buf_call(bufnr, function()
        vim.cmd.write()
      end)
    end)
    if not wrote then
      notify("Failed to save Onda buffer before starting preview.", vim.log.levels.ERROR)
      return
    end
  end

  local cmd = onda_preview_cmd(path)
  local cwd = vim.fn.fnamemodify(path, ":p:h")
  if spawn_detached(cmd, cwd) then
    return
  end

  local job_id = vim.fn.jobstart(cmd, { cwd = cwd, detach = true })
  if job_id <= 0 then
    notify("Failed to start Onda preview.", vim.log.levels.ERROR)
  end
end

function M.setup(opts)
  state.opts = vim.tbl_deep_extend("force", vim.deepcopy(defaults), state.opts, opts or {})
  if state.opts.preview_host ~= nil
    and state.opts.preview_host ~= "egui"
    and state.opts.preview_host ~= "webview"
  then
    notify("Invalid Onda preview_host; expected 'egui', 'webview', or nil.", vim.log.levels.ERROR)
    state.opts.preview_host = nil
  end
  if state.initialized then
    return
  end
  state.initialized = true

  local group = vim.api.nvim_create_augroup("OndaNvim", { clear = true })

  apply_highlights()

  vim.api.nvim_create_autocmd("FileType", {
    group = group,
    pattern = "onda",
    callback = function(args)
      M.start_lsp(args.buf)
    end,
  })

  vim.api.nvim_create_autocmd("ColorScheme", {
    group = group,
    callback = function()
      apply_highlights()
    end,
  })

  vim.api.nvim_create_user_command("OndaRunPatch", function()
    M.run_patch()
  end, {
    desc = "Run the current Onda patch in the standalone preview window",
  })
end

return M
