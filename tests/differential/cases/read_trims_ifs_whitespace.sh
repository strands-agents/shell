# read strips leading and trailing IFS whitespace when a name is given (#343)
printf '  a b  \n' | { read x; echo "[$x]"; }
printf '  a b  \n' | { read x y; echo "[$x][$y]"; }
printf '\t a  b  c \t\n' | { read x y; echo "[$x][$y]"; }
printf '  42\n' | { read n; [ "$n" = 42 ] && echo eq || echo "ne [$n]"; }
printf '   \n' | { read x; echo "[$x]"; }
printf '  a\n' | { read -d '' x; echo "[$x]"; }
printf '  a b  ,' | { read -d , x y; echo "[$x][$y]"; }
# REPLY and an empty IFS keep the whitespace
printf '  a b  \n' | { read; echo "[$REPLY]"; }
printf '  a b  \n' | { IFS=; read x; echo "[$x]"; }
# only whitespace that is in IFS is stripped
printf ' \ta\t \n' | { IFS=' '; read x; echo "[$x]"; }
