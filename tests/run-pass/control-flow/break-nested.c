// `break` leaves the innermost loop only, so the outer one runs all three of
// its iterations and each inner one stops after adding 1.
//@ exit-code: 3

int main() {
    int count = 0;

    for (int i = 0; i < 3; i = i + 1) {
        for (int j = 0; j < 10; j = j + 1) {
            count = count + 1;
            if (j == 0) {
                break;
            }
            count = count + 100;
        }
    }

    return count;
}
