set -e
for i in 1 2; do { false; break; } || echo x; done
false
echo unreachable
