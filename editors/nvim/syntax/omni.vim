if exists("b:current_syntax")
  finish
endif

syn case match

syn match omniComment "#.*$"
syn region omniString start=+"+ skip=+\\\\\|\\"+ end=+"+
syn match omniNumber "\<\d\+\%(\.\d\+\)\?\%([eE][+-]\=\d\+\)\?\>"

syn match omniSection "^\s*\zs\%(ins\|outs\|params\|const\|events\|buffers\|init\|block\|sample\|graph\)\ze\%(\s*<\|\s\+\d\+\|\s*:\|\s*{\)"
syn match omniImportKeyword "^\s*\zs\%(import\|include\)\ze\>"
syn match omniImportPath "^\s*import\s\+\zs[A-Za-z_][A-Za-z0-9_]*\%(/[A-Za-z_][A-Za-z0-9_]*\)*"

syn match omniNamespaceDecl "^\s*\zsnamespace\ze\>"
syn match omniTypeDecl "^\s*\zs\%(proc\|processor\|struct\)\ze\>" nextgroup=omniTypeName skipwhite
syn match omniDefDecl "^\s*\zsdef\ze\>" nextgroup=omniFunctionName skipwhite
syn match omniConstDecl "\<const\>"
syn match omniTypeName "[A-Za-z_][A-Za-z0-9_]*" contained
syn match omniFunctionName "[A-Za-z_][A-Za-z0-9_]*" contained

syn keyword omniKeyword if elif else for in while loop break continue return assert
syn keyword omniBoolean true false
syn keyword omniType f32 f64 i32 i64 bool buffer

syn match omniRate "@\%(sample\|block\)\>"
syn match omniGraphOperator ">>\[[^]\n]\+\]\|<<\[[^]\n]\+\]\|>>\|<<"
syn match omniRangeOperator "\.\.=\|\.\."
syn match omniOperator "::\|==\|!=\|<=\|>=\|&&"
syn match omniOperator "||"
syn match omniOperator "[+*/%=&|^~!<>-]"

hi def link omniComment Comment
hi def link omniString String
hi def link omniNumber Number
hi def link omniSection Keyword
hi def link omniImportKeyword Include
hi def link omniImportPath String
hi def link omniNamespaceDecl Keyword
hi def link omniTypeDecl Keyword
hi def link omniDefDecl Keyword
hi def link omniConstDecl Keyword
hi def link omniTypeName Type
hi def link omniFunctionName Function
hi def link omniKeyword Statement
hi def link omniBoolean Boolean
hi def link omniType Type
hi def link omniRate PreProc
hi def link omniGraphOperator Operator
hi def link omniRangeOperator Operator
hi def link omniOperator Operator

let b:current_syntax = "omni"
