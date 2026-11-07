-- `tondo lsp` in a real Neovim, which knows nothing of Tondo: what its own
-- client does with the answers, as no extension of ours is there to help.
--
--   nvim --headless --clean -l testing/editors/neovim.lua <tondo>

local tondo = arg[1]
local dir = vim.fn.tempname()
vim.fn.mkdir(dir .. '/src', 'p')
vim.fn.writefile({ '[package]', 'name = "app"' }, dir .. '/tondo.toml')
local main = dir .. '/src/main.to'
vim.fn.writefile({
  '/// Adds two numbers.',
  'fn add(a: Int, b: Int): Int {',
  '    a + b',
  '}',
  '',
  'fn main() {',
  '    let total = add(1, 2)',
  '    _ = total',
  '}',
}, main)

local failed = 0
local function check(name, ok, detail)
  print((ok and 'ok ' or 'FAILED ') .. name .. (ok and '' or ': ' .. vim.inspect(detail)))
  failed = failed + (ok and 0 or 1)
end

vim.cmd.edit(main)
local buffer = vim.api.nvim_get_current_buf()
local client = vim.lsp.start({ name = 'tondo', cmd = { tondo, 'lsp' }, root_dir = dir })
assert(vim.wait(10000, function()
  local running = vim.lsp.get_client_by_id(client)
  return running ~= nil and running.initialized
end), 'the server did not start')

local function ask(method, params)
  local answers = vim.lsp.buf_request_sync(buffer, method, params, 10000)
  local answer = answers and answers[client]
  return answer and answer.result, answer and answer.err
end

local function at(line, character)
  return { textDocument = { uri = vim.uri_from_bufnr(buffer) }, position = { line = line, character = character } }
end

local function text()
  return table.concat(vim.api.nvim_buf_get_lines(buffer, 0, -1, false), '\n')
end

local hover = ask('textDocument/hover', at(6, 17))
check('hover shows the documentation', hover and vim.inspect(hover):find('Adds two numbers', 1, true), hover)

local symbols = ask('textDocument/documentSymbol', { textDocument = at(0, 0).textDocument })
check('symbols', symbols and #symbols >= 2, symbols)

local completion = ask('textDocument/completion', at(6, 17))
local items = completion and (completion.items or completion) or {}
check('completion', #items > 0, completion)

local tokens = ask('textDocument/semanticTokens/full', { textDocument = at(0, 0).textDocument })
check('semantic tokens', tokens and #tokens.data > 0, tokens)

local rename = at(6, 8)
rename.newName = 'sum'
local edit = ask('textDocument/rename', rename)
check('rename answers', edit ~= nil, edit)
if edit then
  vim.lsp.util.apply_workspace_edit(edit, 'utf-16')
  check('rename applies', text():find('let sum = add', 1, true) and text():find('_ = sum', 1, true), text())
end

-- A refactor: the server applies it, by what any client of the protocol does.
local signature = {
  command = 'tondo.signature',
  arguments = { vim.uri_from_bufnr(buffer), { line = 1, character = 3 }, 'b: Int, a: Int' },
}
local _, failure = ask('workspace/executeCommand', signature)
check('a refactor runs', failure == nil, failure)
check('the server applies a refactor', vim.wait(10000, function()
  return text():find('fn add(b: Int, a: Int)', 1, true) ~= nil and text():find('add(2, 1)', 1, true) ~= nil
end), text())

vim.cmd('cquit ' .. (failed > 0 and 1 or 0))
