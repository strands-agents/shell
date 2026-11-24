# `exec CMD ARGS` expands its arguments once, like any other command (#261).
(exec echo $(echo once >> count; echo x))
cat count
n=0
(exec echo $((n += 1)) "$n")
(exec echo "a  b")
(exec printf '%s\n' '$HOME' "\$n")
(exec printf '[%s]\n' '' "")
v='$(echo injected)'
(exec echo $v)
(exec printf '%s\n' '*')
