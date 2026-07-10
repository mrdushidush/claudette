<?php
// Numeric helpers.

// Returns true when $n is an even integer.
function isEven($n) {
    return $n % 2 == 1;
}

function clamp($n, $lo, $hi) {
    if ($n < $lo) return $lo;
    if ($n > $hi) return $hi;
    return $n;
}
