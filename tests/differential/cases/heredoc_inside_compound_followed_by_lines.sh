if true; then
  cat <<EOF
then body
EOF
  echo more
fi
while read l; do
  cat <<IN
in $l
IN
done <<OUT
a
b
OUT
if cat <<EOF
cond
EOF
then
  echo yes
fi
for i in 1; do
  case $i in
    1) { cat <<EOF; }
nested $i
EOF
    ;;
  esac
done
f() {
  cat <<EOF
)
fi
}
EOF
}
f
x=$(cat <<EOF
sub
EOF
)
echo "[$x]"
echo after
