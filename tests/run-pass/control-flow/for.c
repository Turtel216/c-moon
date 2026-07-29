// A `for` loop runs its init clause once, tests the condition before every
// iteration and its step after each one: 0 + 1 + 2 + 3 + 4.
//@ exit-code: 10

int main() {
    int sum = 0;

    for (int i = 0; i < 5; i = i + 1) {
        sum = sum + i;
    }

    return sum;
}
