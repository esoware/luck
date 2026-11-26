// The C ABI that ffi.rs declares. Every function is noexcept: an exception
// Analysis documents, an InternalCompilerError or one of its kinds, is caught
// and reported, and any other, such as std::bad_alloc, terminates the
// process, as running out of memory does in Rust.

#include "Luau/Ast.h"
#include "Luau/BuiltinDefinitions.h"
#include "Luau/Common.h"
#include "Luau/Config.h"
#include "Luau/ConfigResolver.h"
#include "Luau/Error.h"
#include "Luau/ExperimentalFlags.h"
#include "Luau/FileResolver.h"
#include "Luau/Frontend.h"
#include "Luau/TypeArena.h"
#include "Luau/TypeCheckLimits.h"

#include <cstdint>
#include <cstring>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <utility>
#include <vector>

extern "C"
{

struct LuckStr
{
    const char* data;
    size_t len;
};

struct LuckHost
{
    void* context;
    bool (*source)(void* context, LuckStr module, void* source);
    bool (*resolve)(void* context, LuckStr requirer, LuckStr path, void* module);
    void (*config)(void* context, LuckStr module, void* config);
};

struct LuckError
{
    size_t module;
    bool is_syntax_error;
    bool has_location;
    uint32_t begin_line;
    uint32_t begin_column;
    uint32_t end_line;
    uint32_t end_column;
    LuckStr module_name;
    LuckStr message;
};

struct LuckErrors
{
    void* context;
    void (*push)(void* context, const LuckError* error);
};

struct LuckExecutor
{
    void* context;
    void (*submit)(void* context, void* task);
};

enum : uint8_t
{
    LUCK_CHECKED,
    LUCK_CANCELLED,
    LUCK_INTERNAL_ERROR,
};

} // extern "C"

namespace
{

using CancellationToken = std::shared_ptr<Luau::FrontendCancellationToken>;

LuckStr str(const std::string& value)
{
    return {value.data(), value.size()};
}

// What luau-analyze runs with: every flag of Luau's own that Luau doesn't list
// as experimental. Flags are process-wide, so they're set once, before the
// first frontend reads them.
void setFlags()
{
    static const bool isSet = []
    {
        for (Luau::FValue<bool>* flag = Luau::FValue<bool>::list; flag; flag = flag->next)
            if (std::strncmp(flag->name, "Luau", 4) == 0 && !Luau::isAnalysisFlagExperimental(flag->name))
                flag->value = true;
        return true;
    }();
    (void)isSet;
}

struct FileResolver final : Luau::FileResolver
{
    explicit FileResolver(LuckHost host)
        : host(host)
    {
    }

    std::optional<Luau::SourceCode> readSource(const Luau::ModuleName& name) override
    {
        Luau::SourceCode code{{}, Luau::SourceCode::Module};
        if (!host.source(host.context, str(name), &code.source))
            return std::nullopt;
        return code;
    }

    std::optional<Luau::ModuleInfo> resolveModule(const Luau::ModuleInfo* context, Luau::AstExpr* expr, const Luau::TypeCheckLimits&) override
    {
        const Luau::AstExprConstantString* path = expr->as<Luau::AstExprConstantString>();
        if (!context || !path)
            return std::nullopt;

        Luau::ModuleInfo info;
        if (!host.resolve(host.context, str(context->name), {path->value.data, path->value.size}, &info.name))
            return std::nullopt;
        return info;
    }

    LuckHost host;
};

// Analysis copies what it needs out of a config before it asks for the next
// one, so one config serves every module in turn.
struct ConfigResolver final : Luau::ConfigResolver
{
    explicit ConfigResolver(LuckHost host)
        : host(host)
    {
    }

    const Luau::Config& getConfig(const Luau::ModuleName& name, const Luau::TypeCheckLimits&) const override
    {
        config = Luau::Config{};
        host.config(host.context, str(name), &config);
        return config;
    }

    LuckHost host;
    mutable Luau::Config config;
};

struct Checker
{
    explicit Checker(LuckHost host)
        : fileResolver(host)
        , configResolver(host)
        , frontend(Luau::SolverMode::New, &fileResolver, &configResolver)
    {
        Luau::registerBuiltinGlobals(frontend, frontend.globals);
        Luau::freeze(frontend.globals.globalTypes);
    }

    FileResolver fileResolver;
    ConfigResolver configResolver;
    Luau::Frontend frontend;
};

void push(
    LuckErrors errors,
    size_t module,
    bool isSyntaxError,
    const std::string& moduleName,
    const std::optional<Luau::Location>& location,
    const std::string& message
)
{
    LuckError error{module, isSyntaxError, location.has_value(), 0, 0, 0, 0, str(moduleName), str(message)};
    if (location)
    {
        error.begin_line = location->begin.line;
        error.begin_column = location->begin.column;
        error.end_line = location->end.line;
        error.end_column = location->end.column;
    }
    errors.push(errors.context, &error);
}

void pushTypeError(Checker& checker, LuckErrors errors, size_t module, const Luau::TypeError& error)
{
    bool isSyntaxError = Luau::get_if<Luau::SyntaxError>(&error.data) != nullptr;
    std::string message = Luau::toString(error, Luau::TypeErrorToStringOptions{&checker.fileResolver});
    push(errors, module, isSyntaxError, error.moduleName, error.location, message);
}

} // namespace

extern "C"
{

void luck_string_assign(void* string, LuckStr value) noexcept
{
    static_cast<std::string*>(string)->assign(value.data, value.len);
}

// As luau-analyze reads one: without alias options, an alias would stop the
// parse before the keys after it.
void luck_config_apply_luaurc(void* config, LuckStr text, LuckErrors errors) noexcept
{
    Luau::ConfigOptions options;
    options.aliasOptions = Luau::ConfigOptions::AliasOptions{std::nullopt, true};
    if (std::optional<std::string> error = Luau::parseConfig(std::string(text.data, text.len), *static_cast<Luau::Config*>(config), options))
        push(errors, 0, false, std::string(), std::nullopt, *error);
}

void* luck_checker_new(LuckHost host) noexcept
{
    setFlags();
    return new Checker(host);
}

void luck_checker_free(void* checker) noexcept
{
    delete static_cast<Checker*>(checker);
}

void luck_checker_load_definitions(void* raw, LuckStr name, LuckStr source, LuckErrors errors) noexcept
{
    Checker& checker = *static_cast<Checker*>(raw);
    Luau::Frontend& frontend = checker.frontend;
    std::string packageName(name.data, name.len);

    Luau::unfreeze(frontend.globals.globalTypes);
    try
    {
        Luau::LoadDefinitionFileResult result =
            frontend.loadDefinitionFile(frontend.globals, frontend.globals.globalScope, {source.data, source.len}, packageName, false);
        for (const Luau::ParseError& error : result.parseResult.errors)
            push(errors, 0, true, packageName, error.getLocation(), error.getMessage());
        if (result.module)
            for (const Luau::TypeError& error : result.module->errors)
                pushTypeError(checker, errors, 0, error);
    }
    catch (const Luau::InternalCompilerError& error)
    {
        push(errors, 0, false, packageName, error.location, error.message);
    }
    Luau::freeze(frontend.globals.globalTypes);
}

void luck_checker_mark_dirty(void* checker, LuckStr module) noexcept
{
    static_cast<Checker*>(checker)->frontend.markDirty(std::string(module.data, module.len));
}

uint8_t luck_checker_check(
    void* raw,
    const LuckStr* modules,
    size_t count,
    const void* cancellation,
    LuckExecutor executor,
    LuckErrors errors
) noexcept
{
    Checker& checker = *static_cast<Checker*>(raw);
    Luau::Frontend& frontend = checker.frontend;

    std::vector<Luau::ModuleName> names;
    names.reserve(count);
    for (size_t i = 0; i < count; ++i)
        names.emplace_back(modules[i].data, modules[i].len);

    Luau::FrontendOptions options = frontend.options;
    options.cancellationToken = *static_cast<const CancellationToken*>(cancellation);

    try
    {
        frontend.queueModuleCheck(names);
        frontend.checkQueuedModules(
            options,
            [executor](std::vector<std::function<void()>> tasks)
            {
                for (std::function<void()>& task : tasks)
                    executor.submit(executor.context, new std::function<void()>(std::move(task)));
            }
        );
        if (options.cancellationToken->requested())
            return LUCK_CANCELLED;

        for (size_t i = 0; i < count; ++i)
            if (std::optional<Luau::CheckResult> result = frontend.getCheckResult(names[i], false))
                for (const Luau::TypeError& error : result->errors)
                    pushTypeError(checker, errors, i, error);
        return LUCK_CHECKED;
    }
    catch (const Luau::UserCancelError&)
    {
        return LUCK_CANCELLED;
    }
    catch (const Luau::InternalCompilerError& error)
    {
        push(errors, count, false, error.moduleName.value_or(std::string()), error.location, error.message);
        return LUCK_INTERNAL_ERROR;
    }
}

void luck_task_run(void* task) noexcept
{
    std::unique_ptr<std::function<void()>> owned(static_cast<std::function<void()>*>(task));
    (*owned)();
}

void* luck_cancellation_new() noexcept
{
    return new CancellationToken(std::make_shared<Luau::FrontendCancellationToken>());
}

void luck_cancellation_cancel(const void* token) noexcept
{
    (*static_cast<const CancellationToken*>(token))->cancel();
}

void luck_cancellation_free(void* token) noexcept
{
    delete static_cast<CancellationToken*>(token);
}

LuckStr luck_builtin_definitions() noexcept
{
    setFlags();
    static const std::string source = Luau::getBuiltinDefinitionSource();
    return str(source);
}

} // extern "C"
