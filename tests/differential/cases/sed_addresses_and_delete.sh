printf '1\n2\n3\n4\n' | sed '2,3d'
printf '1\n2\n3\n4\n' | sed -n '/3/,$p'
printf 'keep\n# drop\nkeep2\n' | sed '/^#/d'
