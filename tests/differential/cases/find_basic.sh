mkdir -p d/a/b
touch d/x.txt d/a/y.txt d/a/b/z.log
find d -name '*.txt' | sort
find d -type d | sort
find d -type f -name 'z*'
