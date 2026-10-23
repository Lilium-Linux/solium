#include "lock.h"

#include <QtCore/QByteArray>
#include <QtCore/QCoreApplication>
#include <QtCore/QJsonArray>
#include <QtCore/QJsonDocument>
#include <QtCore/QJsonObject>
#include <QtCore/QJsonParseError>
#include <QtCore/QMetaMethod>
#include <QtCore/QMetaProperty>
#include <QtQml/QQmlContext>
#include <QtQml/QQmlEngine>

#include <algorithm>
#include <cmath>

void SoliumLock::submit()
{
    m_requests |= 1;
}

void SoliumLock::clear()
{
    m_requests |= 2;
}

int SoliumLock::takeRequests()
{
    const int taken = m_requests;
    m_requests = 0;
    return taken;
}

void SoliumLock::publish(const QJsonObject &next)
{
    const QString state = next.value(QStringLiteral("state")).toString(m_state);
    const int attempts = next.value(QStringLiteral("attempts")).toInt(m_attempts);
    const int retry_after = next.value(QStringLiteral("retryAfter")).toInt(m_retry_after);
    const QString message = next.value(QStringLiteral("message")).toString(m_message);
    const QVariantMap user = next.value(QStringLiteral("user")).toObject().toVariantMap();
    const QString prompt_on = next.value(QStringLiteral("promptOn")).toString(m_prompt_on);

    const bool state_moved = state != m_state;
    const bool attempts_moved = attempts != m_attempts;
    const bool retry_moved = retry_after != m_retry_after;
    const bool message_moved = message != m_message;
    const bool user_moved = user != m_user;
    const bool prompt_moved = prompt_on != m_prompt_on;
    m_state = state;
    m_attempts = attempts;
    m_retry_after = retry_after;
    m_message = message;
    m_user = user;
    m_prompt_on = prompt_on;

    if (attempts_moved) {
        emit attemptsChanged();
    }
    if (retry_moved) {
        emit retryAfterChanged();
    }
    if (message_moved) {
        emit messageChanged();
    }
    if (user_moved) {
        emit userChanged();
    }
    if (prompt_moved) {
        emit promptOnChanged();
    }
    // Last, so a handler of the state reads the counts and the line that go
    // with it.
    if (state_moved) {
        emit stateChanged();
    }
}

void SoliumSecretState::publish(const QJsonObject &field, const QJsonObject &keypad)
{
    const int next_length = field.value(QStringLiteral("length")).toInt(length);
    const bool next_selected = field.value(QStringLiteral("selected")).toBool(selected);
    const bool next_accepting = field.value(QStringLiteral("accepting")).toBool(accepting);
    const bool field_moved =
        next_length != length || next_selected != selected || next_accepting != accepting;
    length = next_length;
    selected = next_selected;
    accepting = next_accepting;

    const QString next_layout = keypad.value(QStringLiteral("layout")).toString(layout);
    QList<QList<SoliumPadKey>> next_rows;
    for (const QJsonValue &row : keypad.value(QStringLiteral("rows")).toArray()) {
        QList<SoliumPadKey> keys;
        for (const QJsonValue &key : row.toArray()) {
            const QJsonObject each = key.toObject();
            keys.append(SoliumPadKey{each.value(QStringLiteral("label")).toString(),
                                     each.value(QStringLiteral("letters")).toString(),
                                     each.value(QStringLiteral("kind")).toString()});
        }
        next_rows.append(keys);
    }
    const bool keypad_moved = next_layout != layout || next_rows != rows;
    layout = next_layout;
    rows = next_rows;

    const bool next_pressed = keypad.value(QStringLiteral("pressed")).toBool(pressed);
    const bool pressed_moved = next_pressed != pressed;
    pressed = next_pressed;
    // A count, not a list of keys: how many were pressed is all a scene is
    // told, so its dots can bounce. Sent as a double by JSON, exact well past
    // any number of presses a session makes.
    const auto next_presses = static_cast<unsigned long long>(
        std::max(0.0, keypad.value(QStringLiteral("presses")).toDouble(0.0)));
    const unsigned long long fresh = next_presses > presses ? next_presses - presses : 0;
    presses = next_presses;

    if (keypad_moved) {
        emit keypadChanged();
    }
    if (field_moved) {
        emit fieldChanged();
    }
    if (pressed_moved) {
        emit pressedChanged();
    }
    for (unsigned long long each = 0; each < fresh && each < 64; ++each) {
        emit keyPressed();
    }
}

SoliumLock *solium_lock()
{
    static SoliumLock *instance = nullptr;
    if (instance == nullptr) {
        instance = new SoliumLock();
        QQmlEngine::setObjectOwnership(instance, QQmlEngine::CppOwnership);
    }
    return instance;
}

SoliumSecretState *solium_secret_state()
{
    static SoliumSecretState *instance = nullptr;
    if (instance == nullptr) {
        instance = new SoliumSecretState();
    }
    return instance;
}

SoliumSecretField::SoliumSecretField(QQuickItem *parent) : QQuickItem(parent)
{
    SoliumSecretState *state = solium_secret_state();
    connect(state, &SoliumSecretState::fieldChanged, this, &SoliumSecretField::follow);
    m_length = state->length;
    m_selected = state->selected;
    m_accepting = state->accepting;
}

int SoliumSecretField::length() const
{
    return m_length;
}

bool SoliumSecretField::selected() const
{
    return m_selected;
}

bool SoliumSecretField::accepting() const
{
    return m_accepting;
}

void SoliumSecretField::follow()
{
    const SoliumSecretState *state = solium_secret_state();
    const bool length_moved = state->length != m_length;
    const bool selected_moved = state->selected != m_selected;
    const bool accepting_moved = state->accepting != m_accepting;
    m_length = state->length;
    m_selected = state->selected;
    m_accepting = state->accepting;
    if (accepting_moved) {
        emit acceptingChanged();
    }
    if (selected_moved) {
        emit selectedChanged();
    }
    if (length_moved) {
        emit lengthChanged();
    }
}

SoliumSecretKeypad::SoliumSecretKeypad(QQuickItem *parent) : QQuickItem(parent)
{
    SoliumSecretState *state = solium_secret_state();
    connect(state, &SoliumSecretState::keypadChanged, this, [this] {
        emit layoutChanged();
        if (isComponentComplete()) {
            rebuild();
        }
    });
    connect(state, &SoliumSecretState::pressedChanged, this, &SoliumSecretKeypad::pressedChanged);
    connect(state, &SoliumSecretState::keyPressed, this, &SoliumSecretKeypad::keyPressed);
}

SoliumSecretKeypad::~SoliumSecretKeypad()
{
    for (const QPointer<QQuickItem> &key : m_keys) {
        delete key.data();
    }
}

void SoliumSecretKeypad::setDelegate(QQmlComponent *delegate)
{
    if (delegate == m_delegate) {
        return;
    }
    m_delegate = delegate;
    emit delegateChanged();
    if (isComponentComplete()) {
        rebuild();
    }
}

QString SoliumSecretKeypad::layout() const
{
    return solium_secret_state()->layout;
}

bool SoliumSecretKeypad::pressed() const
{
    return solium_secret_state()->pressed;
}

void SoliumSecretKeypad::componentComplete()
{
    QQuickItem::componentComplete();
    rebuild();
}

void SoliumSecretKeypad::geometryChange(const QRectF &next, const QRectF &old)
{
    QQuickItem::geometryChange(next, old);
    place();
}

QRectF SoliumSecretKeypad::cell(int row, int column) const
{
    // Every row as tall as the others, and every key in a row as wide as the
    // others in it: the one rule both the delegates and `keyAt` follow.
    const int rows = static_cast<int>(m_lengths.size());
    if (rows == 0 || row < 0 || row >= rows) {
        return {};
    }
    const int columns = m_lengths.at(row);
    if (columns <= 0) {
        return {};
    }
    const qreal height = this->height() / rows;
    const qreal width = this->width() / columns;
    return {column * width, row * height, width, height};
}

void SoliumSecretKeypad::rebuild()
{
    for (const QPointer<QQuickItem> &key : m_keys) {
        delete key.data();
    }
    m_keys.clear();
    m_lengths.clear();
    const SoliumSecretState *state = solium_secret_state();
    for (const QList<SoliumPadKey> &row : state->rows) {
        m_lengths.append(static_cast<int>(row.size()));
    }
    if (m_delegate == nullptr) {
        return;
    }
    QQmlContext *context = qmlContext(this);
    for (int row = 0; row < state->rows.size(); ++row) {
        const QList<SoliumPadKey> &keys = state->rows.at(row);
        for (int column = 0; column < keys.size(); ++column) {
            const SoliumPadKey &key = keys.at(column);
            QObject *made = m_delegate->beginCreate(context);
            if (made == nullptr) {
                continue;
            }
            m_delegate->setInitialProperties(made, QVariantMap{
                                                       {QStringLiteral("label"), key.label},
                                                       {QStringLiteral("letters"), key.letters},
                                                       {QStringLiteral("kind"), key.kind},
                                                       {QStringLiteral("row"), row},
                                                       {QStringLiteral("column"), column},
                                                   });
            auto *item = qobject_cast<QQuickItem *>(made);
            if (item == nullptr) {
                m_delegate->completeCreate();
                delete made;
                continue;
            }
            item->setParentItem(this);
            m_delegate->completeCreate();
            m_keys.append(item);
        }
    }
    place();
}

void SoliumSecretKeypad::place()
{
    int at = 0;
    for (int row = 0; row < m_lengths.size(); ++row) {
        for (int column = 0; column < m_lengths.at(row); ++column, ++at) {
            if (at >= m_keys.size()) {
                return;
            }
            QQuickItem *key = m_keys.at(at);
            if (key == nullptr) {
                continue;
            }
            const QRectF rect = cell(row, column);
            key->setPosition(rect.topLeft());
            key->setSize(rect.size());
        }
    }
}

int SoliumSecretKeypad::keyAt(QPointF local) const
{
    if (local.x() < 0 || local.y() < 0 || local.x() >= width() || local.y() >= height()) {
        return -1;
    }
    const int rows = static_cast<int>(m_lengths.size());
    if (rows == 0) {
        return -1;
    }
    const int row = std::min(rows - 1, static_cast<int>(std::floor(local.y() / (height() / rows))));
    const int columns = m_lengths.at(row);
    if (columns <= 0) {
        return -1;
    }
    const int column =
        std::min(columns - 1, static_cast<int>(std::floor(local.x() / (width() / columns))));
    int index = 0;
    for (int before = 0; before < row; ++before) {
        index += m_lengths.at(before);
    }
    return index + column;
}

namespace {

/* Whether `item` is drawn at all: visible, enabled, and not faded out by
 * itself or anything over it. */
bool shown(const QQuickItem *item)
{
    if (!item->isVisible() || !item->isEnabled()) {
        return false;
    }
    for (const QQuickItem *each = item; each != nullptr; each = each->parentItem()) {
        if (each->opacity() <= 0.0) {
            return false;
        }
    }
    return true;
}

} // namespace

int solium_keypad_at(QQuickItem *root, QPointF at)
{
    if (root == nullptr) {
        return -1;
    }
    if (auto *pad = qobject_cast<SoliumSecretKeypad *>(root); pad != nullptr && shown(pad)) {
        const int key = pad->keyAt(pad->mapFromScene(at));
        if (key >= 0) {
            return key;
        }
    }
    for (QQuickItem *child : root->childItems()) {
        const int key = solium_keypad_at(child, at);
        if (key >= 0) {
            return key;
        }
    }
    return -1;
}

/* What Rust publishes once a frame when it moved: the lock's state, the
 * field and the keypad. 0 when Qt has not started, so it is sent again. */
extern "C" int solium_qml_lock_publish(const char *json)
{
    if (QCoreApplication::instance() == nullptr || json == nullptr) {
        return 0;
    }
    QJsonParseError parsed{};
    const QJsonDocument document = QJsonDocument::fromJson(QByteArray(json), &parsed);
    if (parsed.error != QJsonParseError::NoError || !document.isObject()) {
        return 0;
    }
    const QJsonObject next = document.object();
    solium_secret_state()->publish(next.value(QStringLiteral("field")).toObject(),
                                   next.value(QStringLiteral("keypad")).toObject());
    solium_lock()->publish(next);
    return 1;
}

/* `Lock.submit()` and `Lock.clear()` since the last call, as bits. */
extern "C" int solium_qml_lock_take_requests()
{
    if (QCoreApplication::instance() == nullptr) {
        return 0;
    }
    return solium_lock()->takeRequests();
}

/* Every method and property a type declares itself, one per line, for the
 * tests that nothing in QML can unlock: `method <name>`, `signal <name>`, and
 * `property <name> <read-only|writable>`. "lock", "field" or "keypad".
 * `lock_auth::tests::nothing_in_qml_can_unlock_or_read_the_secret`. */
extern "C" const char *solium_qml_lock_api_for_test(const char *which)
{
    static QByteArray text;
    const QMetaObject *meta = nullptr;
    const QByteArray name(which != nullptr ? which : "");
    if (name == "lock") {
        meta = &SoliumLock::staticMetaObject;
    } else if (name == "field") {
        meta = &SoliumSecretField::staticMetaObject;
    } else if (name == "keypad") {
        meta = &SoliumSecretKeypad::staticMetaObject;
    }
    text.clear();
    if (meta == nullptr) {
        return text.constData();
    }
    for (int index = meta->methodOffset(); index < meta->methodCount(); ++index) {
        const QMetaMethod method = meta->method(index);
        text += method.methodType() == QMetaMethod::Signal ? "signal " : "method ";
        text += method.name();
        text += '\n';
    }
    for (int index = meta->propertyOffset(); index < meta->propertyCount(); ++index) {
        const QMetaProperty property = meta->property(index);
        text += "property ";
        text += property.name();
        text += property.isWritable() ? " writable\n" : " read-only\n";
    }
    return text.constData();
}
