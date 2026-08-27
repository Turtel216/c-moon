// Statements after a `break` or a `continue` are unreachable, so neither the
// return nor the addition below ever runs.
//@ exit-code: 4

int main() {
    int n = 0;

    while (1) {
        n = n + 1;
        break;
        return 100;
    }

    for (int i = 0; i < 3; i = i + 1) {
        n = n + 1;
        continue;
        n = n + 100;
    }

    return n;
}
