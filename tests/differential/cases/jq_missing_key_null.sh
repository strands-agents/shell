echo '{"a":1}' | jq '.b'
echo '{"a":1}' | jq -r '.b // "default"'
