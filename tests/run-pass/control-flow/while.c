// A counting loop runs until its condition fails.
//@ exit-code: 10

int main() {
    int i = 0;
    while (i < 10) {
        i = i + 1;
    }

    return i;
}
