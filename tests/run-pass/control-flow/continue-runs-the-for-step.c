// A `continue` in a `for` still runs the step before the next test. Jumping
// straight back to the condition would leave `i` at 2 for ever, so this loop
// terminating at all is the assertion: 0 + 1 + 3 + 4.
//@ exit-code: 8

int main() {
    int sum = 0;

    for (int i = 0; i < 5; i = i + 1) {
        if (i == 2) {
            continue;
        }
        sum = sum + i;
    }

    return sum;
}
