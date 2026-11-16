printf 'a\nb' > p
printf 'a\nc\n' > q
diff p q
diff -u p q | tail -n +3
