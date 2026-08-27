// A loop with no condition of its own is left by its `break` alone, whether
// the condition is omitted entirely or written as a constant.
//@ exit-code: 7

int main() {
    int n = 0;

    for (;;) {
        n = n + 1;
        if (n == 3) {
            break;
        }
    }

    while (1) {
        n = n + 2;
        if (n > 5) {
            break;
        }
    }

    return n;
}
