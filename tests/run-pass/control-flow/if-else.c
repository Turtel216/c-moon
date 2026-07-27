// The taken branch returns; the else branch and the trailing statement are
// never reached.
//@ exit-code: 1

int main() {
    int a = 1;
    if (a < 10) {
        return a;
    } else {
        return 2;
    }

    return 3;
}
