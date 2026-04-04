if exists("b:current_syntax")
  finish
endif

syn case match

syn match ondaComment "#.*$"
syn region ondaString start=+"+ skip=+\\\\\|\\"+ end=+"+
syn match ondaNumber "\<\d\+\%(\.\d\+\)\?\%([eE][+-]\=\d\+\)\?\>"

syn match ondaSection "^\s*\zs\%(ins\|outs\|params\|const\|events\|buffers\|init\|block\|sample\|graph\)\ze\%(\s*<\|\s\+\d\+\|\s*:\|\s*{\|\s\+[A-Za-z_(]\)"
syn match ondaImportKeyword "^\s*\zs\%(import\|include\)\ze\>"
syn match ondaImportPath "^\s*import\s\+\zs[A-Za-z_][A-Za-z0-9_]*\%(/[A-Za-z_][A-Za-z0-9_]*\)*"

syn match ondaNamespaceDecl "^\s*\zsnamespace\ze\>"
syn match ondaTypeDecl "^\s*\zs\%(proc\|processor\|struct\)\ze\>" nextgroup=ondaTypeName skipwhite
syn match ondaDefDecl "^\s*\zsdef\ze\>" nextgroup=ondaFunctionName skipwhite
syn match ondaConstDecl "\<const\>"
syn match ondaTypeName "[A-Za-z_][A-Za-z0-9_]*" contained
syn match ondaFunctionName "[A-Za-z_][A-Za-z0-9_]*" contained

syn keyword ondaKeyword if elif else for in while loop break continue return assert
syn keyword ondaBoolean true false
syn keyword ondaType f32 f64 i32 i64 bool buffer

syn match ondaRate "@\%(sample\|block\)\>"
syn match ondaGraphOperator ">>\[[^]\n]\+\]\|<<\[[^]\n]\+\]\|>>\|<<"
syn match ondaRangeOperator "\.\.=\|\.\."
syn match ondaOperator "::\|==\|!=\|<=\|>=\|&&"
syn match ondaOperator "||"
syn match ondaOperator "[+*/%=&|^~!<>-]"

hi def link ondaComment Comment
hi def link ondaString String
hi def link ondaNumber Number
hi def link ondaSection Keyword
hi def link ondaImportKeyword Include
hi def link ondaImportPath String
hi def link ondaNamespaceDecl Keyword
hi def link ondaTypeDecl Keyword
hi def link ondaDefDecl Keyword
hi def link ondaConstDecl Keyword
hi def link ondaTypeName Type
hi def link ondaFunctionName Function
hi def link ondaKeyword Statement
hi def link ondaBoolean Boolean
hi def link ondaType Type
hi def link ondaRate PreProc
hi def link ondaGraphOperator Operator
hi def link ondaRangeOperator Operator
hi def link ondaOperator Operator

let b:current_syntax = "onda"
