// A `char` controls an `if` and a `while` the way any integer does: the test
// asks whether it is zero.
//@ exit-code: 5

int main() {
    char c = 'a';
    char stop = 'f';
    int steps = 0;

    while (c != stop) {
        c = c + 1;
        steps = steps + 1;
    }

    char zero = 0;
    if (zero) {
        return 1;
    }
    if (c) {
        return steps;
    }
    return 2;
}
