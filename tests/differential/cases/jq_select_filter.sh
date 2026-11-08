echo '[{"n":"a","v":1},{"n":"b","v":5}]' | jq -r '.[] | select(.v > 2) | .n'
