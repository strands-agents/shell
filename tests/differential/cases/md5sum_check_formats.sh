printf 'hello\n' > a
h=b1946ac92492d2347c6235b4d2611184
H=B1946AC92492D2347C6235B4D2611184
printf '%s a\n' "$h" > unmarked
md5sum -c unmarked
echo "status=$?"
printf '%s  a\n%s a\n' "$h" "$h" > marked-first
md5sum -c marked-first 2>/dev/null
echo "status=$?"
printf '%s a\n%s  a\n' "$h" "$h" > unmarked-first
md5sum -c unmarked-first 2>/dev/null
echo "status=$?"
printf ' \t%s  a\n' "$h" > leading-blanks
md5sum -c leading-blanks
echo "status=$?"
printf '%s\t a\n' "$h" > tab-separator
md5sum -c tab-separator
echo "status=$?"
printf '%s  a\n' "$H" > upper
md5sum -c upper
echo "status=$?"
printf '%s  -\n' "$h" | md5sum -c - 2>/dev/null
echo "status=$?"
printf '\\%s  a\\qb\n' "$h" > bad-escape
md5sum -c bad-escape 2>/dev/null
echo "status=$?"
printf '\\%s  ab\\\n' "$h" > trailing-backslash
md5sum -c trailing-backslash 2>/dev/null
echo "status=$?"
