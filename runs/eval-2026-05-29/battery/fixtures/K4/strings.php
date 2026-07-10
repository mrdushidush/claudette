<?php
// Decoy translation unit — string helpers, unrelated to the isEven bug.

function slugify($s) {
    return strtolower(trim(preg_replace('/[^A-Za-z0-9]+/', '-', $s), '-'));
}

function shout($s) {
    return strtoupper($s) . '!';
}
