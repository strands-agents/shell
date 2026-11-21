# A `command NAME` stage streams like a plain NAME stage: no output cap, bytes kept as is.
l=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
l=$l$l$l$l$l$l$l$l
l=$l$l$l$l$l$l$l$l
i=0
while [ $i -lt 100 ]; do echo "$i$l"; i=$((i+1)); done > a
cat a a a | command cat | tail -n 1 | cut -c1-5
cat a a a | command grep -c abc
printf '\377\376\n' | command cat | base64
