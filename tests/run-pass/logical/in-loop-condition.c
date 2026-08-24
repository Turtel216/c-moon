// A short-circuiting condition branches on its way to a value, so a loop
// whose condition contains one is tested in a different block than the one
// its condition started in.
//@ exit-code: 20

int main() {
    int total = 0;
    int i = 0;
    int enabled = 1;

    while (i < 10 && enabled) {
        total = total + 1;
        i = i + 1;
    }

    for (i = 0; i < 10 || enabled; i = i + 1) {
        total = total + 1;
        if (i == 9) {
            enabled = 0;
        }
    }

    return total;
}
