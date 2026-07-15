set -eu

export HOME="$TMPDIR/home"
mkdir -p "$HOME" upstream/ignored source scratch
git config --global user.email test@example.com
git config --global user.name Test

git -C upstream init --quiet
printf 'ignored/\n' >upstream/.gitignore
printf 'tracked despite ignore\n' >upstream/ignored/tracked.txt
git -C upstream add .gitignore
git -C upstream add --force ignored/tracked.txt
git -C upstream commit --quiet -m base
expected_tree=$(git -C upstream rev-parse 'HEAD^{tree}')

git -C upstream checkout-index --all --prefix="$PWD/source/"
cp "$dagLib" scratch/dag-lib.nu
cp "$seedTestScript" scratch/seed-test.nu
nu scratch/seed-test.nu "$PWD/source" "$expected_tree"

touch "$out"
