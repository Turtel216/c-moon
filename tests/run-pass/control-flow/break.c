// `break` leaves the loop at once, so the counter stops where the test inside
// the body says rather than where the loop's own condition would.
//@ exit-code: 3

int main() {
    int i = 0;

    while (i < 100) {
        if (i == 3) {
            break;
        }
        i = i + 1;
    }

    return i;
}
