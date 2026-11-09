v='a:b::c'
IFS=:
for w in $v; do echo "[$w]"; done
