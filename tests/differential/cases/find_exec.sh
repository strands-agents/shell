mkdir d
echo hi > d/one.txt
find d -name '*.txt' -exec cat {} \;
