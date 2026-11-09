cat does-not-exist 2>/dev/null
echo "status=$?"
cat does-not-exist >/dev/null 2>&1 || echo failed quietly
