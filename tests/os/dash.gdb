# dashboard styling
dash -layout assembly source stack variables
dash source -style highlight-line True
dash source -style height 15
dash source -style path True
dash stack -style limit 5
dash assembly -style height 7
dash assembly -style function False
dash assembly -style opcodes True
dash assembly -style highlight-line True
dash variables -style align True
dash variables -style compact False
dash variables -style sort True
dash -enabled on

# 设置汇编语法
set disassembly-flavor intel
# 显示人类可读的函数名（这个貌似需要在 GDB 运行之后执行）
set print asm-demangle on
