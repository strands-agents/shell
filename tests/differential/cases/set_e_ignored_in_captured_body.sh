set -e
f() { false; echo in-f; }
x=$(f || echo caught); echo "x=$x"
{ f || echo c; } > out.txt; cat out.txt
{ f && echo ok; } 2>/dev/null
(f || echo c) | cat
for i in 1 2; do f || echo c; done | cat
if true; then f || echo c; fi | cat
echo after
