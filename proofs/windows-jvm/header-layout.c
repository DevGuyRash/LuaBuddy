/* Build-time oracle only: never linked into or shipped with LuaBuddy. Read
 * the chosen JDK's C ABI through its named fields, independent of Rust. */
#include <jvmti.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

_Static_assert(sizeof(void *) == 8, "expected 64-bit pointers");
_Static_assert(sizeof(jint) == 4 && sizeof(jlong) == 8 && sizeof(jlocation) == 8,
               "unexpected JNI/JVMTI integer widths");
_Static_assert(sizeof(jvmtiCapabilities) == 4 * sizeof(uint32_t), "capabilities size");
/* The Rust callable table is the JVMTI 1.2 prefix, not the whole JDK 25 table. */
_Static_assert(sizeof(jvmtiEventCallbacks) >= 37 * sizeof(void *), "callback prefix");
#define EVENT(field, index) _Static_assert(offsetof(jvmtiEventCallbacks, field) == (index) * sizeof(void *), #field)
EVENT(VMInit, 0);
EVENT(ClassFileLoadHook, 4);
EVENT(ClassPrepare, 6);
EVENT(Breakpoint, 12);
#define SLOT(field, one_based) _Static_assert(offsetof(struct jvmtiInterface_1_, field) == ((one_based)-1) * sizeof(void *), #field)
SLOT(SetEventNotificationMode, 2);
SLOT(GetLocalObject, 21);
SLOT(GetLocalInt, 22);
SLOT(GetLocalLong, 23);
SLOT(SetLocalInt, 27);
SLOT(SetBreakpoint, 38);
SLOT(ClearBreakpoint, 39);
SLOT(Allocate, 46);
SLOT(Deallocate, 47);
SLOT(GetClassSignature, 48);
SLOT(GetClassMethods, 52);
SLOT(GetMethodName, 64);
SLOT(GetMethodDeclaringClass, 65);
SLOT(GetMethodModifiers, 66);
SLOT(GetBytecodes, 75);
SLOT(ForceEarlyReturnInt, 82);
SLOT(SetEventCallbacks, 122);
SLOT(AddCapabilities, 142);

int main(void) {
    const uint32_t expected[4] = {
        (1u << 3) | (1u << 14) | (1u << 19) | (1u << 26), 1u << 1, 0, 0
    };
    jvmtiCapabilities named;
    memset(&named, 0, sizeof named);
    named.can_get_bytecodes = 1;
    named.can_access_local_variables = 1;
    named.can_generate_breakpoint_events = 1;
    named.can_generate_all_class_hook_events = 1;
    named.can_force_early_return = 1;
    if (memcmp(&named, expected, sizeof named) != 0) {
        fputs("JVMTI capabilities do not match the Rust x64 boundary\n", stderr);
        return 1;
    }
    puts("PASS JDK JNI/JVMTI x64 header layout and capability bitfields");
    return 0;
}
