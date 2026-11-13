printf 'hello\n' > a
md5sum does-not-exist a
echo "status=$?"
