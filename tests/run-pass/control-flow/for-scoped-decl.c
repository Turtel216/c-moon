// A variable declared in the init clause stays visible to the condition, the
// step and the body, and goes out of scope at the end of the loop -- so the
// same name can be declared again afterwards.
//@ exit-code: 7

int main() {
    for (int i = 0; i < 3; i = i + 1) {
        int doubled = i + i;
    }

    int i = 7;
    return i;
}
