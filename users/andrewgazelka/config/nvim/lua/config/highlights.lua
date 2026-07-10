local M = {}

local function set(group, opts)
  vim.api.nvim_set_hl(0, group, opts)
end

local function notion_palette()
  if vim.o.background == "light" then
    return {
      fg = "#37352f",
      muted = "#787774",
      code_bg = "#f1f1ef",
      inline_bg = "#e8e8e6",
      inline_fg = "#eb5757",
      link = "#337ea9",
    }
  end

  return {
    fg = "#e9e9e7",
    muted = "#9b9a97",
    code_bg = "#242424",
    inline_bg = "#2f2f2f",
    inline_fg = "#ff7369",
    link = "#7f9cf5",
  }
end

function M.apply_transparency()
  for _, group in ipairs({
    "Normal",
    "NormalFloat",
    "NormalNC",
    "SignColumn",
    "EndOfBuffer",
  }) do
    set(group, { bg = "none" })
  end
end

function M.apply_markdown_notion()
  local palette = notion_palette()

  set("MarkdownNormal", { fg = palette.fg, bg = "none" })
  set("MarkdownEndOfBuffer", { fg = palette.fg, bg = "none" })

  set("RenderMarkdownCode", { fg = palette.fg, bg = palette.code_bg })
  set("RenderMarkdownCodeInline", { fg = palette.inline_fg, bg = palette.inline_bg })
  set("RenderMarkdownCodeInfo", { fg = palette.muted, bg = palette.code_bg, italic = true })
  set("RenderMarkdownCodeFallback", { fg = palette.muted, bg = palette.code_bg })
  set("RenderMarkdownCodeBorder", { fg = palette.code_bg, bg = "none" })

  set("markdownCode", { fg = palette.inline_fg, bg = palette.inline_bg })
  set("markdownCodeBlock", { fg = palette.fg, bg = "none" })
  set("markdownCodeDelimiter", { fg = palette.muted, bg = "none" })
  set("markdownHeadingDelimiter", { fg = palette.muted, bg = "none" })
  set("markdownBold", { fg = palette.fg, bold = true })
  set("markdownItalic", { fg = palette.fg, italic = true })
  set("markdownUrl", { fg = palette.link, underline = true })
  set("markdownLinkText", { fg = palette.link, underline = true })
  set("markdownBlockquote", { fg = palette.muted, italic = true })
  set("markdownListMarker", { fg = palette.fg })

  for _, group in ipairs({
    "@markup.raw",
    "@markup.raw.block",
  }) do
    set(group, { bg = "none" })
  end

  for _, group in ipairs({
    "@markup.raw.markdown",
    "@markup.raw.block.markdown",
  }) do
    set(group, { fg = palette.fg, bg = "none" })
  end

  set("@markup.raw.markdown_inline", { fg = palette.inline_fg, bg = palette.inline_bg })

  for _, group in ipairs({
    "@markup.heading.1.markdown",
    "@markup.heading.2.markdown",
    "@markup.heading.3.markdown",
    "@markup.heading.4.markdown",
    "@markup.heading.5.markdown",
    "@markup.heading.6.markdown",
    "@markup.heading.markdown",
  }) do
    set(group, { fg = palette.fg, bg = "none", bold = true })
  end

  set("@markup.strong.markdown_inline", { fg = palette.fg, bold = true })
  set("@markup.italic.markdown_inline", { fg = palette.fg, italic = true })
  set("@markup.strikethrough.markdown_inline", { fg = palette.muted, strikethrough = true })
  set("@markup.link.markdown_inline", { fg = palette.link, underline = true })
  set("@markup.link.label.markdown_inline", { fg = palette.link, underline = true })
  set("@markup.link.url.markdown_inline", { fg = palette.muted, underline = true })
  set("@markup.list.markdown", { fg = palette.fg })
  set("@markup.quote.markdown", { fg = palette.muted, italic = true })
  set("@punctuation.special.markdown", { fg = palette.muted })
  set("@punctuation.delimiter.markdown", { fg = palette.muted })
  set("@punctuation.bracket.markdown_inline", { fg = palette.muted })
end

function M.apply_all()
  M.apply_transparency()
  M.apply_markdown_notion()
end

return M
