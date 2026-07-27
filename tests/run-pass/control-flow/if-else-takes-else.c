// The else branch runs when the condition fails.
//@ exit-code: 2

int main() {
    int a = 20;
    if (a < 10) {
        return 1;
    } else {
        return 2;
    }

    return 3;
}
