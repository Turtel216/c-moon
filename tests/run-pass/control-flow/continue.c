// `continue` abandons the rest of the body and tests the condition again, so
// only the last five iterations reach the addition: 6 + 7 + 8 + 9 + 10.
//@ exit-code: 40

int main() {
    int i = 0;
    int sum = 0;

    while (i < 10) {
        i = i + 1;
        if (i < 6) {
            continue;
        }
        sum = sum + i;
    }

    return sum;
}
