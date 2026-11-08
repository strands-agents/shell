v='a  b   c'
for w in $v; do echo "[$w]"; done
for w in "$v"; do echo "[$w]"; done
