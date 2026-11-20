foo() { echo "foo ran"; }
"" foo
echo "status=$?"
unset u
"$u" foo
echo "status=$?"
