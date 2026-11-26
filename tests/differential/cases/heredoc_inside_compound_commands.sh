name=world
if true; then cat <<EOF
then $name
EOF
fi
if false; then :; else cat <<EOF
else $name
EOF
fi
n=0
while [ $n -lt 2 ]; do cat <<EOF
while $n
EOF
n=$((n + 1)); done
for i in a b; do cat <<EOF
for $i
EOF
done
until [ $n -eq 0 ]; do cat <<EOF
until $n
EOF
n=0; done
case x in x) cat <<EOF
case $name
EOF
;; esac
{ cat <<EOF
group $name
EOF
}
( cat <<EOF
subshell $name
EOF
)
f() { cat <<EOF
function $1
EOF
}
f arg
echo after
