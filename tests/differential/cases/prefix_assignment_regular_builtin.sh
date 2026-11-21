# A prefix assignment on a regular builtin applies to that command only (#308).
printf 'a,b\n' | { IFS=, read x y; echo "[$x][$y]"; }
printf 'a b\n' | { IFS=, read x y; echo "[$x][$y]"; }
printf 'a:b\nc:d e\n' | { IFS=: read x y; read p q; echo "[$x][$y][$p][$q]"; }
v=old
printf 'new\n' | { v=tmp read v; echo "[$v]"; }
unset u
u=1 true
echo "[${u-unset}]"
