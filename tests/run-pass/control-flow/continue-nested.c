// `continue` repeats the innermost loop only. The inner one skips its second
// iteration and the outer one is untouched, so the body runs 3 * 2 times.
//@ exit-code: 6

int main() {
    int count = 0;

    for (int i = 0; i < 3; i = i + 1) {
        for (int j = 0; j < 3; j = j + 1) {
            if (j == 1) {
                continue;
            }
            count = count + 1;
        }
    }

    return count;
}
