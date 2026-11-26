g() { cat; }
x=$( { echo a; } | g ); echo "[$x]"
{ { echo b; } | g; } > f; echo "file:$(cat f)"
for i in 1 2; do { echo $i; } | g; done | tr 12 xy
y=$( (echo c) | g ); echo "[$y]"
