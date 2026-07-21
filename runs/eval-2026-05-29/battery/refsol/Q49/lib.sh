# Helper library for solution.sh

# Strip surrounding whitespace from the single argument and echo the result.
normalize() {
  echo "$1" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//'
}
