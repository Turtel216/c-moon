// Nested loops: each `j` loop restarts from its own init clause, so the body
// runs 3 * 2 times.
//@ exit-code: 6

int main() {
    int count = 0;

    for (int i = 0; i < 3; i = i + 1) {
        for (int j = 0; j < 2; j = j + 1) {
            count = count + 1;
        }
    }

    return count;
}
