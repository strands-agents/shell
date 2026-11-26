if command -v ls >/dev/null 2>&1; then echo have-ls; fi
command -v cd > out
echo "file: $(cat out)"
f() { :; }
command -v f >> out
echo "appended: $(cat out)"
command -V cd > out
echo "verbose: $(cat out)"
command -v no-such-cmd-xyz > out
echo "missing $? [$(cat out)]"
command -v cd 3>&1 >/dev/null
echo end
