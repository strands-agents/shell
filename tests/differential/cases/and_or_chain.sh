true && echo a || echo b
false && echo c || echo d
false || false || echo e
true && false && echo f
echo "status=$?"
