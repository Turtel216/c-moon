// The right operand of `||` is evaluated only when the left one leaves the
// answer open, which is observable through the side effect `bump` has.
//@ exit-code: 111

int bump(int *counter) {
    *counter = *counter + 1;
    return 1;
}

int main() {
    int calls = 0;
    int zero = 0;
    int one = 1;

    // A true left operand settles it, so `bump` never runs.
    if (one || bump(&calls)) {
        calls = calls + 10;
    }
    // A false one does not, so `bump` runs exactly once.
    if (zero || bump(&calls)) {
        calls = calls + 100;
    }

    return calls;
}
