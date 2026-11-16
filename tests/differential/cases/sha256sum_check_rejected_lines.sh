h=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
printf '%s  -\n' "$h" | sha256sum -c - 2>/dev/null
echo "status=$?"
printf '\\%s  a\\qb\n' "$h" > bad-escape
sha256sum -c bad-escape 2>/dev/null
echo "status=$?"
printf '\\%s  ab\\\n' "$h" > trailing-backslash
sha256sum -c trailing-backslash 2>/dev/null
echo "status=$?"
