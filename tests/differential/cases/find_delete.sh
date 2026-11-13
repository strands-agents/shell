mkdir -p d/keep d/gone/sub start/in
touch d/a.txt d/b.log d/keep/c.txt d/gone/sub/e start/in/f
find d -name '*.txt' -delete
find d -name gone -delete
echo "status=$?"
find d -path 'd/gone*' -delete
find d | sort
find start -delete
test -e start || echo "start removed"
