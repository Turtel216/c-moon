// A loop whose condition is false on entry leaves its variable untouched.
//@ exit-code: 5

int main() {
    int i = 5;
    while (i < 5) {
        i = i + 1;
    }

    return i;
}
