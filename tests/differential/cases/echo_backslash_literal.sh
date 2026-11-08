# known-difference: echo expands backslash escapes by default; bash prints them literally
echo 'a\tb'
echo "c:\new"
