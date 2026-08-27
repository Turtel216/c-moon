// A `break` in a `for` skips the step as well as the rest of the body, so the
// loop is left with `i` still at 4 and 0 + 1 + 2 + 3 added.
//@ exit-code: 10

int main() {
    int sum = 0;
    int i;

    for (i = 0; i < 100; i = i + 1) {
        if (i == 4) {
            break;
        }
        sum = sum + i;
    }

    return sum + i;
}
