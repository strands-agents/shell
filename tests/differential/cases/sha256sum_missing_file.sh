printf 'hello\n' > a
sha256sum does-not-exist a
echo "status=$?"
