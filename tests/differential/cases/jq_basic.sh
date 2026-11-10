echo '{"a":[1,2,3],"b":"text","c":{"d":true}}' > data.json
jq '.a | length' data.json
jq -r '.b' data.json
jq -c '.a | map(. * 2)' data.json
jq '.c.d' data.json
jq -r '.a[]' data.json
