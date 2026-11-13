printf 'hello\n' > a
h=5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03
printf '%s  a\n%s a\n' "$h" "$h" > marked-first
sha256sum -c marked-first 2>/dev/null
echo "status=$?"
printf '%s a\n%s  a\n' "$h" "$h" > unmarked-first
sha256sum -c unmarked-first 2>/dev/null
echo "status=$?"
